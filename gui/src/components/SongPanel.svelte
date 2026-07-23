<script lang="ts">
  import { app, send } from "../lib/store.svelte";

  const song = $derived(app.snap!.song);
</script>

<section class="song">
  <div class="col">
    <div class="chead">
      <h2>Scenes</h2>
      <button onclick={() => send({ type: "captureScene" })} title="Capture current lane state as a new scene">
        + Capture
      </button>
    </div>
    {#if song.scenes.length === 0}
      <p class="muted small">No scenes yet. Capture the current lanes to make one.</p>
    {:else}
      <div class="list">
        {#each song.scenes as s (s.index)}
          <div class="scene">
            <span class="idx mono">{s.index + 1}</span>
            <span class="nm">{s.name}</span>
            <button onclick={() => send({ type: "recallScene", args: s.index })}>Recall</button>
          </div>
        {/each}
      </div>
    {/if}
  </div>

  <div class="col">
    <h2>Chains</h2>
    {#if song.chains.length === 0}
      <p class="muted small">
        No chains yet. Chains sequence scenes into a song — create them in the TUI
        (<code>midip</code>) for now; playback works here.
      </p>
    {:else}
      <div class="list">
        {#each song.chains as c (c.index)}
          {@const playing = song.playing_chain === c.index}
          <div class="chain" class:playing>
            <div class="chain-head">
              <span class="nm">{c.name}</span>
              {#if c.looped}<span class="loop" title="Looping">↻</span>{/if}
              <div class="spacer"></div>
              {#if playing}
                <button class="stop" onclick={() => send({ type: "stopChain" })}>■ Stop</button>
              {:else}
                <button class="play" onclick={() => send({ type: "playChain", args: c.index })}>▶ Play</button>
              {/if}
            </div>
            <div class="entries">
              {#each c.entries as e, i (i)}
                <span class="entry" class:cur={c.current_entry === i}>
                  {e.scene}<span class="rep mono">×{e.repeats}</span>
                </span>
              {/each}
            </div>
          </div>
        {/each}
      </div>
    {/if}
  </div>
</section>

<style>
  .song {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 20px;
    padding: 16px;
    overflow-y: auto;
    height: 100%;
  }
  .chead {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  h2 {
    font-size: 12px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--ember);
    margin: 0 0 8px;
  }
  .list {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .scene {
    display: flex;
    align-items: center;
    gap: 10px;
    border: var(--border);
    border-radius: var(--radius);
    padding: 6px 8px;
    background: var(--panel);
  }
  .idx {
    color: var(--dim);
    min-width: 18px;
  }
  .nm {
    flex: 1;
    color: var(--fg);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .scene .nm {
    flex: 1;
  }
  .chain {
    border: var(--border);
    border-left: 3px solid var(--dim);
    border-radius: var(--radius);
    padding: 8px;
    background: var(--panel);
  }
  .chain.playing {
    border-left-color: var(--ok);
  }
  .chain-head {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .loop {
    color: var(--aqua);
  }
  .spacer {
    flex: 1;
  }
  .play {
    color: var(--ok);
  }
  .stop {
    color: var(--err);
  }
  .entries {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    margin-top: 8px;
  }
  .entry {
    font-size: 11px;
    color: var(--fg-dim);
    border: 1px solid var(--dim-2);
    border-radius: 2px;
    padding: 2px 6px;
  }
  .entry.cur {
    color: var(--bg);
    background: var(--ok);
    border-color: var(--ok);
  }
  .rep {
    color: var(--dim);
    margin-left: 4px;
    font-size: 10px;
  }
  .entry.cur .rep {
    color: var(--bg);
  }
  .small {
    font-size: 11px;
  }
  code {
    color: var(--aqua);
  }
</style>
