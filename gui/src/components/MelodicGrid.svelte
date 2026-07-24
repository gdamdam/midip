<script lang="ts">
  import { app, send, placeNote } from "../lib/store.svelte";
  import { midiName } from "../lib/music";

  const pat = $derived(app.snap!.focused_pattern);
  const sel = $derived(app.snap!.selection);
  const t = $derived(app.snap!.transport);
  const lane = $derived(app.snap!.focused_lane);
  const cols = $derived(Array.from({ length: pat.length }, (_, i) => i));

  // Pitch window: bracket the notes present, with a comfortable margin, anchored
  // on the lane root so an empty pattern still shows a usable range.
  const pitchRows = $derived.by(() => {
    let lo = pat.root - 2;
    let hi = pat.root + 14;
    for (const step of pat.melodic_steps) {
      for (const n of step) {
        lo = Math.min(lo, n.pitch - 2);
        hi = Math.max(hi, n.pitch + 2);
      }
    }
    lo = Math.max(0, lo);
    hi = Math.min(127, hi);
    const rows: number[] = [];
    for (let p = hi; p >= lo; p--) rows.push(p);
    return rows;
  });

  // The note at a given pitch in a step, if any. A step may hold several
  // simultaneous notes (a chord), so we match by pitch rather than taking the
  // first — every voice of a chord must render on its own row.
  function noteAt(col: number, pitch: number) {
    return pat.melodic_steps[col]?.find((n) => n.pitch === pitch);
  }
  function isBlack(pitch: number): boolean {
    return [1, 3, 6, 8, 10].includes(((pitch % 12) + 12) % 12);
  }

  const playCol = $derived(t.playing ? t.playhead % Math.max(pat.length, 1) : -1);

  // Click an empty cell: place a note at that row's pitch (scale-folded by the
  // engine). Placing in an occupied column replaces the note (mono lanes).
  function place(col: number, pitch: number) {
    placeNote(lane, col, pitch);
  }
  // Remove the note in a column via toggle (it clears when a note is present).
  function clearCol(col: number) {
    send({ type: "clearStep", args: { lane, row: 0, col } });
  }

  // Vertical drag on the selected step nudges pitch by scale degree.
  let dragCol = $state(-1);
  let dragY = $state(0);
  const ROW_PX = 15;

  function noteDown(e: MouseEvent, col: number) {
    e.stopPropagation();
    send({ type: "selectStep", args: { lane, row: 0, col } }); // select without editing
    dragCol = col;
    dragY = e.clientY;
  }
  function onMove(e: MouseEvent) {
    if (dragCol < 0) return;
    const dy = e.clientY - dragY;
    if (Math.abs(dy) >= ROW_PX) {
      const steps = Math.trunc(dy / ROW_PX);
      for (let i = 0; i < Math.abs(steps); i++) {
        send({ type: steps < 0 ? "noteUp" : "noteDown", args: { lane, col: dragCol } });
      }
      dragY = e.clientY;
    }
  }
  function endDrag() {
    dragCol = -1;
  }
  $effect(() => {
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", endDrag);
    return () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", endDrag);
    };
  });
</script>

<div class="scroll">
  <div class="pianoroll">
    <!-- step-number header -->
    <div class="hrow">
      <div class="corner"></div>
      {#each cols as c (c)}
        <div class="stepno" class:bar={c % 4 === 0} class:playhead={c === playCol}>
          {c % 4 === 0 ? c + 1 : "·"}
        </div>
      {/each}
    </div>

    {#each pitchRows as pitch (pitch)}
      <div class="prow" class:black={isBlack(pitch)} class:root={pitch === pat.root}>
        <div class="keylabel mono">{midiName(pitch)}</div>
        {#each cols as c (c)}
          {@const n = noteAt(c, pitch)}
          {@const on = !!n}
          <button
            class="mcell"
            class:sep={c % 4 === 3 && c !== pat.length - 1}
            class:playhead={c === playCol}
            class:selected={sel.col === c && on}
            onclick={() => !on && place(c, pitch)}
            aria-label={`step ${c + 1} pitch ${midiName(pitch)}`}
          >
            {#if on}
              <span
                class="note"
                class:slide={n!.slide}
                style:opacity={0.4 + 0.6 * n!.vel}
                onmousedown={(e) => noteDown(e, c)}
                ondblclick={() => clearCol(c)}
                role="button"
                tabindex="-1"
                title="{midiName(pitch)} — drag to change pitch, double-click to remove"
              ></span>
            {/if}
          </button>
        {/each}
      </div>
    {/each}
  </div>
</div>

<style>
  .scroll {
    overflow: auto;
    height: 100%;
    padding: 8px;
  }
  .pianoroll {
    display: inline-block;
    min-width: min-content;
  }
  .hrow,
  .prow {
    display: flex;
    align-items: center;
    gap: 2px;
  }
  .corner,
  .keylabel {
    position: sticky;
    left: 0;
    z-index: 2;
    width: 46px;
    flex: 0 0 46px;
    background: var(--bg);
    font-size: 10px;
    color: var(--dim);
    text-align: right;
    padding-right: 6px;
    height: 14px;
    line-height: 14px;
  }
  .prow.root .keylabel {
    color: var(--pink);
  }
  .stepno {
    width: var(--step);
    flex: 0 0 var(--step);
    text-align: center;
    font-size: 10px;
    color: var(--dim);
    font-variant-numeric: tabular-nums;
  }
  .stepno.bar {
    color: var(--fg-dim);
  }
  .stepno.playhead {
    color: var(--pink);
  }
  .mcell {
    position: relative;
    width: var(--step);
    height: 14px;
    flex: 0 0 var(--step);
    padding: 0;
    border: none;
    background: var(--bg);
    border-top: 1px solid #00000000;
  }
  .prow.black .mcell {
    background: #1a1d1e;
  }
  .prow.root .mcell {
    background: #232726;
  }
  .mcell.sep {
    margin-right: 6px;
  }
  .mcell.playhead {
    background: var(--playhead);
  }
  .note {
    position: absolute;
    inset: 1px;
    background: var(--pink);
    border-radius: 2px;
    cursor: ns-resize;
    display: block;
  }
  .note.slide {
    background: linear-gradient(90deg, var(--pink), var(--ember));
  }
  .mcell.selected .note {
    box-shadow: inset 0 0 0 2px var(--fg);
  }
</style>
