<script lang="ts">
  import type { TrigCond } from "../lib/types";

  interface Props {
    present: boolean;
    /** 0..1 fill intensity (velocity-derived). */
    intensity?: number;
    prob?: number;
    ratchet?: number;
    micro?: number;
    cond?: TrigCond | null;
    selected?: boolean;
    playhead?: boolean;
  }
  let {
    present,
    intensity = 1,
    prob = 1,
    ratchet = 1,
    micro = 0,
    cond = null,
    selected = false,
    playhead = false,
  }: Props = $props();

  // Priority: ratchet > prob > cond > micro (mirrors theme.rs step_attr_marker).
  const marker = $derived(
    ratchet > 1
      ? ratchet <= 9
        ? "²³⁴⁵⁶⁷⁸⁹"[ratchet - 2]
        : "⁺"
      : prob < 1
        ? "°"
        : cond && cond.type !== "Always"
          ? "?"
          : micro !== 0
            ? "≈"
            : "",
  );
</script>

<div class="cell" class:present class:selected class:playhead aria-hidden="true">
  {#if present}
    <div class="fill" style:opacity={0.35 + 0.65 * intensity}></div>
  {/if}
  {#if marker}<span class="marker">{marker}</span>{/if}
</div>

<style>
  .cell {
    position: relative;
    width: var(--step);
    height: var(--step);
    background: var(--bg);
    border-radius: 2px;
    display: flex;
    align-items: center;
    justify-content: center;
    overflow: hidden;
  }
  .cell.playhead {
    background: var(--playhead);
  }
  .fill {
    position: absolute;
    inset: 3px;
    background: var(--accent, var(--ember));
    border-radius: 2px;
  }
  .cell.selected {
    box-shadow: inset 0 0 0 2px var(--fg);
  }
  .marker {
    position: relative;
    font-size: 10px;
    color: var(--bg);
    z-index: 1;
    line-height: 1;
  }
  .cell:not(.present) .marker {
    color: var(--dim);
  }
</style>
