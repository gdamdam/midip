<script lang="ts">
  import { app, send } from "../lib/store.svelte";
  import StepCell from "./StepCell.svelte";
  import type { DrumHit } from "../lib/types";

  const pat = $derived(app.snap!.focused_pattern);
  const sel = $derived(app.snap!.selection);
  const t = $derived(app.snap!.transport);
  const lane = $derived(app.snap!.focused_lane);
  const cols = $derived(Array.from({ length: pat.length }, (_, i) => i));

  function hitAt(row: number, col: number): DrumHit | undefined {
    const note = pat.voices[row]?.note;
    return pat.drum_steps[col]?.find((h) => h.note === note);
  }

  let painting = $state(false);
  let paintFill = $state(true);

  function pointerDown(row: number, col: number) {
    const present = !!hitAt(row, col);
    paintFill = !present; // fill empties, erase filled
    painting = true;
    send({ type: "toggleStep", args: { lane, row, col } });
  }
  function pointerEnter(row: number, col: number) {
    if (!painting) return;
    const present = !!hitAt(row, col);
    if (present !== paintFill) send({ type: "toggleStep", args: { lane, row, col } });
  }
  function stop() {
    painting = false;
  }

  $effect(() => {
    window.addEventListener("mouseup", stop);
    return () => window.removeEventListener("mouseup", stop);
  });

  const playCol = $derived(t.playing ? t.playhead % Math.max(pat.length, 1) : -1);
</script>

<div class="scroll">
  <div class="grid" style="--accent: var(--ember)">
    <!-- step-number header -->
    <div class="hrow">
      <div class="corner"></div>
      {#each cols as c (c)}
        <div class="stepno" class:bar={c % 4 === 0} class:playhead={c === playCol}>
          {c % 4 === 0 ? c + 1 : "·"}
        </div>
      {/each}
    </div>

    {#each pat.voices as voice, row (voice.note)}
      <div class="vrow" class:vmuted={voice.muted}>
        <button
          class="vlabel"
          onclick={() => send({ type: "toggleVoiceMute", args: { lane, row } })}
          title="Mute voice {voice.label}"
        >
          <span class="vname">{voice.label}</span>
          <span class="vnote mono">{voice.note}</span>
        </button>
        {#each cols as c (c)}
          {@const h = hitAt(row, c)}
          <button
            class="cellbtn"
            class:sep={c % 4 === 3 && c !== pat.length - 1}
            onmousedown={() => pointerDown(row, c)}
            onmouseenter={() => pointerEnter(row, c)}
            aria-label={`${voice.label} step ${c + 1}${h ? " on" : " off"}`}
          >
            <StepCell
              present={!!h}
              intensity={h ? h.vel / 127 : 1}
              prob={h?.prob ?? 1}
              ratchet={h?.ratchet ?? 1}
              micro={h?.micro ?? 0}
              cond={h?.cond ?? null}
              selected={sel.row === row && sel.col === c}
              playhead={c === playCol}
            />
          </button>
        {/each}
      </div>
    {/each}
  </div>
</div>

<style>
  .scroll {
    overflow-x: auto;
    overflow-y: auto;
    height: 100%;
    padding: 8px;
  }
  .grid {
    display: inline-block;
    min-width: min-content;
  }
  .hrow,
  .vrow {
    display: flex;
    align-items: center;
    gap: 2px;
  }
  .vrow {
    margin-top: 2px;
  }
  .vrow.vmuted {
    opacity: 0.4;
  }
  .corner,
  .vlabel {
    position: sticky;
    left: 0;
    z-index: 2;
    width: 58px;
    flex: 0 0 58px;
    background: var(--bg);
  }
  .vlabel {
    display: flex;
    justify-content: space-between;
    align-items: center;
    height: var(--step);
    padding: 0 6px;
    color: var(--fg-dim);
    border: var(--border);
    border-radius: var(--radius);
  }
  .vname {
    font-weight: 700;
    font-size: 11px;
  }
  .vnote {
    font-size: 10px;
    color: var(--dim);
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
    color: var(--ember);
  }
  .cellbtn {
    padding: 0;
    border: none;
    background: none;
    flex: 0 0 var(--step);
  }
  .cellbtn.sep {
    margin-right: 6px;
  }
</style>
