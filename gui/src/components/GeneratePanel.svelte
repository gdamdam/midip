<script lang="ts">
  import { app, send } from "../lib/store.svelte";

  // Rendered at the top level (before the snapshot loads), so stay null-safe.
  const gen = $derived(app.snap?.gen ?? null);

  const MODES = [
    { id: "generate", label: "Generate" },
    { id: "vary", label: "Vary" },
    { id: "arp", label: "Arp" },
  ] as const;

  function adjust(field: string, delta: number) {
    send({ type: "genAdjust", args: { field, delta } });
  }
</script>

{#if gen?.active}
  <div class="scrim" role="presentation" onclick={() => send({ type: "genCancel" })}></div>
  <div class="panel" role="dialog" aria-label="Pattern generator">
    <div class="head">
      <h2>Generate</h2>
      <span class="hint muted">live preview — plays as you tweak</span>
    </div>

    <div class="modes">
      {#each MODES as m (m.id)}
        <button
          class="mode"
          class:active={gen.mode === m.id}
          disabled={m.id === "arp" && !gen.melodic}
          title={m.id === "arp" && !gen.melodic ? "Arp is melodic-only" : ""}
          onclick={() => send({ type: "genSetMode", args: m.id })}
        >{m.label}</button>
      {/each}
    </div>

    <div class="params">
      <div class="row">
        <span class="k">Density</span>
        <button onclick={() => adjust("density", -5)}>−</button>
        <span class="v mono">{gen.density}%</span>
        <button onclick={() => adjust("density", 5)}>+</button>
      </div>

      {#if gen.melodic && gen.mode !== "vary"}
        <div class="row">
          <span class="k">Range</span>
          <button onclick={() => adjust("range", -1)}>−</button>
          <span class="v mono">{gen.range} st</span>
          <button onclick={() => adjust("range", 1)}>+</button>
        </div>
      {/if}

      {#if gen.mode === "vary"}
        <div class="row">
          <span class="k">Mutate</span>
          <button onclick={() => adjust("mutate", -5)}>−</button>
          <span class="v mono">{gen.mutate}%</span>
          <button onclick={() => adjust("mutate", 5)}>+</button>
        </div>
      {/if}

      {#if gen.mode === "arp"}
        <div class="arp">
          <div class="row">
            <span class="k">Chord</span>
            <button class="wide" onclick={() => adjust("chord", 1)}>{gen.arp_chord}</button>
          </div>
          <div class="row">
            <span class="k">Octaves</span>
            <button onclick={() => adjust("octaves", -1)}>−</button>
            <span class="v mono">{gen.arp_octaves}</span>
            <button onclick={() => adjust("octaves", 1)}>+</button>
          </div>
          <div class="row">
            <span class="k">Shape</span>
            <button class="wide" onclick={() => adjust("shape", 1)}>{gen.arp_shape}</button>
          </div>
          <div class="row">
            <span class="k">Gate</span>
            <button onclick={() => adjust("gate", -5)}>−</button>
            <span class="v mono">{Math.round(gen.arp_gate * 100)}%</span>
            <button onclick={() => adjust("gate", 5)}>+</button>
          </div>
          <div class="row">
            <span class="k">Vel var</span>
            <button onclick={() => adjust("velvar", -5)}>−</button>
            <span class="v mono">{gen.arp_vel_var}%</span>
            <button onclick={() => adjust("velvar", 5)}>+</button>
          </div>
        </div>
      {/if}
    </div>

    <div class="actions">
      <button class="reroll" onclick={() => send({ type: "genReroll" })}>↻ Re-roll</button>
      <div class="spacer"></div>
      <button class="cancel" onclick={() => send({ type: "genCancel" })}>Cancel</button>
      <button class="commit" onclick={() => send({ type: "genCommit" })}>Commit</button>
    </div>
  </div>
{/if}

<style>
  .scrim {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.45);
    z-index: 60;
  }
  .panel {
    position: fixed;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    width: 340px;
    max-width: 90vw;
    background: var(--panel);
    border: var(--border-strong);
    border-radius: 6px;
    box-shadow: 0 12px 40px rgba(0, 0, 0, 0.5);
    z-index: 61;
    padding: 14px 16px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .head {
    display: flex;
    align-items: baseline;
    gap: 10px;
  }
  h2 {
    margin: 0;
    font-size: 13px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--ember);
  }
  .hint {
    font-size: 10px;
  }
  .modes {
    display: flex;
    gap: 4px;
  }
  .mode {
    flex: 1;
    color: var(--fg-dim);
  }
  .mode.active {
    color: var(--bg);
    background: var(--ember);
    border-color: var(--ember);
    font-weight: 700;
  }
  .params {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .arp {
    display: flex;
    flex-direction: column;
    gap: 8px;
    border-top: var(--border);
    padding-top: 8px;
  }
  .row {
    display: grid;
    grid-template-columns: 1fr auto auto auto;
    align-items: center;
    gap: 6px;
  }
  .row .k {
    font-size: 11px;
    color: var(--fg-dim);
  }
  .row .v {
    min-width: 52px;
    text-align: center;
  }
  .row button {
    padding: 2px 8px;
    min-width: 26px;
  }
  .row .wide {
    grid-column: 2 / 5;
    text-transform: capitalize;
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 6px;
    border-top: var(--border);
    padding-top: 12px;
  }
  .spacer {
    flex: 1;
  }
  .reroll {
    color: var(--pink);
  }
  .commit {
    color: var(--bg);
    background: var(--ok);
    border-color: var(--ok);
    font-weight: 700;
  }
  .cancel {
    color: var(--fg-dim);
  }
</style>
