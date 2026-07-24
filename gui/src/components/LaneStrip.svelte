<script lang="ts">
  import { app, send } from "../lib/store.svelte";
  import { roleColor } from "../lib/music";
  import type { Lane } from "../lib/types";

  const lanes = $derived(app.snap!.lanes);
  const t = $derived(app.snap!.transport);

  function playheadCell(lane: Lane): number {
    return lane.length > 0 ? t.playhead % lane.length : 0;
  }
</script>

<div class="lanes">
  {#each lanes as lane (lane.index)}
    <div
      class="lane"
      role="button"
      tabindex="0"
      aria-pressed={lane.focused}
      aria-label={`Focus ${lane.label} lane`}
      class:focused={lane.focused}
      class:muted={lane.mute}
      style:--accent={roleColor(lane.role)}
      onclick={() => send({ type: "focusLane", args: lane.index })}
      onkeydown={(e) => (e.key === "Enter" || e.key === " ") && send({ type: "focusLane", args: lane.index })}
    >
      <div class="row1">
        <span class="focusmark">{lane.focused ? "▸" : ""}</span>
        <span class="label">{lane.label}</span>
        <span class="patname" title={lane.pattern_name}>{lane.pattern_name || "—"}</span>
        <span class="device" title={`Device: ${lane.device_label}`}>{lane.device_label}</span>
        <span class="conn" class:ok={lane.connected} title={lane.device}>
          {lane.connected ? "●" : "○"}
        </span>
      </div>

      <div class="row2">
        <button
          class="tag mute"
          class:active={lane.mute}
          onclick={(e) => { e.stopPropagation(); send({ type: "toggleMute", args: lane.index }); }}
        >M</button>
        <button
          class="tag solo"
          class:active={lane.solo}
          onclick={(e) => { e.stopPropagation(); send({ type: "toggleSolo", args: lane.index }); }}
        >S</button>
        <span class="ch mono">ch{lane.channel}</span>
        {#if lane.queued}
          <button
            class="queued"
            onclick={(e) => { e.stopPropagation(); send({ type: "cancelQueue", args: lane.index }); }}
            title="Queued — click to cancel"
          >⟶ {lane.queued}</button>
        {/if}
        <div class="spacer"></div>
        <span class="len mono muted">{lane.length}</span>
      </div>

      <!-- activity: playhead position over the lane's own length (polymeter) -->
      <div class="activity" aria-hidden="true">
        {#each Array.from({ length: Math.min(lane.length, 16) }, (_, i) => i) as i (i)}
          <span
            class="cell"
            class:beat={i % 4 === 0}
            class:head={t.playing && i === playheadCell(lane) % 16}
          ></span>
        {/each}
      </div>
    </div>
  {/each}
</div>

<style>
  .lanes {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 8px;
  }
  .lane {
    border: var(--border);
    border-left: 3px solid var(--accent);
    border-radius: var(--radius);
    background: var(--panel);
    padding: 6px 8px;
    cursor: pointer;
    outline: none;
  }
  .lane.focused {
    background: var(--panel-2);
    border-color: var(--accent);
  }
  .lane.muted {
    opacity: 0.55;
  }
  .lane:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
  .row1 {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .focusmark {
    width: 10px;
    color: var(--accent);
    font-weight: 700;
  }
  .label {
    color: var(--accent);
    font-weight: 700;
    font-size: 11px;
    letter-spacing: 0.05em;
    min-width: 44px;
  }
  .patname {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--fg);
  }
  .device {
    font-size: 10px;
    color: var(--fg-dim);
    white-space: nowrap;
    max-width: 40%;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .conn {
    color: var(--dim);
    font-size: 11px;
  }
  .conn.ok {
    color: var(--aqua);
  }
  .row2 {
    display: flex;
    align-items: center;
    gap: 5px;
    margin-top: 5px;
  }
  .tag {
    padding: 1px 6px;
    font-size: 11px;
    font-weight: 700;
    color: var(--fg-dim);
    background: transparent;
  }
  .tag.mute.active {
    color: var(--bg);
    background: var(--err);
    border-color: var(--err);
  }
  .tag.solo.active {
    color: var(--bg);
    background: var(--ok);
    border-color: var(--ok);
  }
  .ch {
    font-size: 11px;
    color: var(--fg-dim);
  }
  .queued {
    font-size: 11px;
    color: var(--warn);
    border-color: var(--warn);
    padding: 1px 6px;
  }
  .spacer {
    flex: 1;
  }
  .len {
    font-size: 11px;
  }
  .activity {
    display: flex;
    gap: 2px;
    margin-top: 6px;
  }
  .cell {
    width: 8px;
    height: 4px;
    background: var(--dim-2);
    border-radius: 1px;
  }
  .cell.beat {
    background: var(--dim);
  }
  .cell.head {
    background: var(--accent);
  }
</style>
