<script lang="ts">
  import { app, send, userPatternCmd } from "../lib/store.svelte";
  import DrumGrid from "./DrumGrid.svelte";
  import MelodicGrid from "./MelodicGrid.svelte";
  import ChordProgressionModal from "./ChordProgressionModal.svelte";

  let saving = $state(false);
  let saveName = $state("");
  let chordModal = $state(false);
  async function saveUserPattern() {
    const n = saveName.trim();
    if (!n) return;
    await userPatternCmd({ type: "saveLanePattern", args: n });
    saving = false;
    saveName = "";
  }

  const pat = $derived(app.snap!.focused_pattern);
  const lane = $derived(app.snap!.focused_lane);
  const laneInfo = $derived(app.snap!.lanes[lane]);
  const sel = $derived(app.snap!.selection);
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
        <select
          class="scalesel"
          value={pat.scale}
          onchange={(e) => {
            const idx = app.scales.indexOf((e.currentTarget as HTMLSelectElement).value);
            if (idx >= 0) send({ type: "setScale", args: { lane, index: idx } });
          }}
          title="Scale"
        >
          {#each app.scales as s (s)}
            <option value={s}>{s}</option>
          {/each}
        </select>
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

    <div class="ctl">
      <span class="lbl">swing</span>
      <button onclick={() => send({ type: "adjustLaneSwing", args: { lane, delta: -5 } })}>−</button>
      <span class="val mono">{laneInfo.swing === null ? "set" : Math.round(laneInfo.swing * 100) + "%"}</span>
      <button onclick={() => send({ type: "adjustLaneSwing", args: { lane, delta: 5 } })}>+</button>
      {#if laneInfo.swing !== null}
        <button class="clr" onclick={() => send({ type: "clearLaneSwing", args: lane })} title="Clear override">×</button>
      {/if}
    </div>
    <div class="ctl">
      <span class="lbl">div</span>
      <button onclick={() => send({ type: "cycleClockDiv", args: lane })}>{laneInfo.clock_div ?? "1"}×</button>
    </div>

    <div class="ctl">
      <span class="lbl">xform</span>
      {#if pat.kind === "drums"}
        <button onclick={() => send({ type: "euclid", args: { lane, row: sel.row, dp: -1, dr: 0 } })} title="Euclid pulses −">E−</button>
        <button onclick={() => send({ type: "euclid", args: { lane, row: sel.row, dp: 1, dr: 0 } })} title="Euclid pulses +">E+</button>
        <button onclick={() => send({ type: "euclid", args: { lane, row: sel.row, dp: 0, dr: 1 } })} title="Euclid rotate">⟳</button>
      {:else}
        <button onclick={() => send({ type: "conformToScale", args: lane })} title="Snap notes to scale">conform</button>
      {/if}
      <button onclick={() => send({ type: "rotateLeft", args: lane })} title="Rotate steps left">◀</button>
      <button onclick={() => send({ type: "rotateRight", args: lane })} title="Rotate steps right">▶</button>
      <button onclick={() => send({ type: "toggleFill", args: lane })} title="Toggle fill variant">fill</button>
      <button onclick={() => send({ type: "commitTransform", args: lane })} title="Commit fill/transform">✓</button>
    </div>

    <div class="spacer"></div>
    {#if saving}
      <input class="savein" placeholder="pattern name" bind:value={saveName}
        onkeydown={(e) => e.key === "Enter" && saveUserPattern()} />
      <button onclick={saveUserPattern}>save</button>
    {:else}
      <button onclick={() => (saving = true)} title="Save this lane's pattern to your library">save→lib</button>
    {/if}
    {#if laneInfo.role === "chords"}
      <button class="chords" onclick={() => (chordModal = true)} title="Type a chord progression (chord names) onto this lane">♪ Chords…</button>
    {/if}
    <button class="gen" onclick={() => send({ type: "openGenerative" })} title="Generate / vary / arpeggiate this lane">⚡ Generate</button>
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

{#if chordModal}
  <ChordProgressionModal {lane} close={() => (chordModal = false)} />
{/if}

<style>
  .patwrap {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
  }
  .toolbar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 10px;
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
  .scalesel {
    background: var(--bg);
    color: var(--fg);
    border: var(--border);
    border-radius: var(--radius);
    font-family: inherit;
    font-size: 11px;
    padding: 2px 4px;
    cursor: pointer;
  }
  .scalesel:hover {
    border-color: var(--fg-dim);
  }
  .spacer {
    /* Force a wrap so the action buttons (save / generate / clear) sit on their
       own row; the parameter groups above wrap freely to fit any width. */
    flex-basis: 100%;
    height: 0;
  }
  .clr {
    padding: 2px 6px;
    color: var(--fg-dim);
  }
  .savein {
    width: 120px;
    font-size: 11px;
  }
  .gen {
    color: var(--ember);
    border-color: var(--dim-2);
    font-size: 11px;
    font-weight: 700;
  }
  .gen:hover {
    border-color: var(--ember);
  }
  .chords {
    color: var(--green);
    border-color: var(--dim-2);
    font-size: 11px;
    font-weight: 700;
  }
  .chords:hover {
    border-color: var(--green);
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
