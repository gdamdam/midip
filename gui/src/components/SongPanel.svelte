<script lang="ts">
  import { app, send, addChainEntry } from "../lib/store.svelte";

  const song = $derived(app.snap!.song);

  // Inline-edit state.
  let renScene = $state<number | null>(null);
  let renChain = $state<number | null>(null);
  let editName = $state("");
  let addScene = $state<Record<number, number>>({}); // chain index -> scene index to add
  let confirmScene = $state<number | null>(null);
  let confirmChain = $state<number | null>(null);

  function startRenScene(i: number, name: string) {
    renScene = i;
    editName = name;
  }
  function commitRenScene() {
    if (renScene !== null && editName.trim())
      send({ type: "renameScene", args: { index: renScene, name: editName.trim() } });
    renScene = null;
  }
  function startRenChain(i: number, name: string) {
    renChain = i;
    editName = name;
  }
  function commitRenChain() {
    if (renChain !== null && editName.trim())
      send({ type: "renameChain", args: { index: renChain, name: editName.trim() } });
    renChain = null;
  }
</script>

<section class="song">
  <div class="col">
    <div class="chead">
      <h2>Scenes</h2>
      <button onclick={() => send({ type: "captureScene" })} title="Capture current lanes as a scene">+ Capture</button>
    </div>
    {#if song.scenes.length === 0}
      <p class="muted small">No scenes yet. Capture the current lanes to make one.</p>
    {:else}
      <div class="list">
        {#each song.scenes as s (s.index)}
          <div class="scene">
            <span class="idx mono">{s.index + 1}</span>
            {#if renScene === s.index}
              <input class="ren" bind:value={editName}
                onkeydown={(e) => e.key === "Enter" && commitRenScene()} onblur={commitRenScene} />
            {:else}
              <button class="nm nmbtn" onclick={() => startRenScene(s.index, s.name)} title="Rename">{s.name}</button>
            {/if}
            <button class="mini" onclick={() => send({ type: "recallScene", args: s.index })}>Recall</button>
            <button class="mini" onclick={() => send({ type: "duplicateScene", args: s.index })} title="Duplicate">⧉</button>
            {#if confirmScene === s.index}
              <button class="mini del" onclick={() => { send({ type: "deleteScene", args: s.index }); confirmScene = null; }}>sure?</button>
            {:else}
              <button class="mini" onclick={() => (confirmScene = s.index)} title="Delete">🗑</button>
            {/if}
          </div>
        {/each}
      </div>
    {/if}
  </div>

  <div class="col">
    <div class="chead">
      <h2>Chains</h2>
      <button onclick={() => send({ type: "createChain" })}>+ New chain</button>
    </div>
    {#if song.chains.length === 0}
      <p class="muted small">No chains yet. Create one, then add scenes to it.</p>
    {:else}
      <div class="list">
        {#each song.chains as c (c.index)}
          {@const playing = song.playing_chain === c.index}
          <div class="chain" class:playing>
            <div class="chain-head">
              {#if renChain === c.index}
                <input class="ren" bind:value={editName}
                  onkeydown={(e) => e.key === "Enter" && commitRenChain()} onblur={commitRenChain} />
              {:else}
                <button class="nm nmbtn" onclick={() => startRenChain(c.index, c.name)} title="Rename">{c.name}</button>
              {/if}
              <button class="mini" class:on={c.looped} onclick={() => send({ type: "toggleChainLoop", args: c.index })} title="Loop">↻</button>
              {#if playing}
                <button class="mini stop" onclick={() => send({ type: "stopChain" })}>■</button>
              {:else}
                <button class="mini play" onclick={() => send({ type: "playChain", args: c.index })}>▶</button>
              {/if}
              {#if confirmChain === c.index}
                <button class="mini del" onclick={() => { send({ type: "deleteChain", args: c.index }); confirmChain = null; }}>sure?</button>
              {:else}
                <button class="mini" onclick={() => (confirmChain = c.index)} title="Delete chain">🗑</button>
              {/if}
            </div>

            <div class="entries">
              {#each c.entries as e, ei (ei)}
                <div class="entry" class:cur={c.current_entry === ei}>
                  <button class="escene" onclick={() => playing && send({ type: "jumpChainEntry", args: ei })} title={playing ? "Jump here" : ""}>{e.scene}</button>
                  <span class="ekv mono">×{e.repeats}</span>
                  <button class="mini" onclick={() => send({ type: "setChainEntryRepeats", args: { chain: c.index, entry: ei, value: Math.max(1, e.repeats - 1) } })}>−</button>
                  <button class="mini" onclick={() => send({ type: "setChainEntryRepeats", args: { chain: c.index, entry: ei, value: e.repeats + 1 } })}>+</button>
                  <span class="ekv mono">{e.bars}b</span>
                  <button class="mini" onclick={() => send({ type: "setChainEntryBars", args: { chain: c.index, entry: ei, value: Math.max(1, e.bars - 1) } })}>−</button>
                  <button class="mini" onclick={() => send({ type: "setChainEntryBars", args: { chain: c.index, entry: ei, value: e.bars + 1 } })}>+</button>
                  <button class="mini" onclick={() => send({ type: "moveChainEntry", args: { chain: c.index, entry: ei, up: true } })} title="Move up">↑</button>
                  <button class="mini" onclick={() => send({ type: "moveChainEntry", args: { chain: c.index, entry: ei, up: false } })} title="Move down">↓</button>
                  <button class="mini del" onclick={() => send({ type: "removeChainEntry", args: { chain: c.index, entry: ei } })}>✕</button>
                </div>
              {/each}
            </div>

            {#if song.scenes.length > 0}
              <div class="addentry">
                <select bind:value={addScene[c.index]}>
                  {#each song.scenes as s (s.index)}
                    <option value={s.index}>{s.name}</option>
                  {/each}
                </select>
                <button class="mini" onclick={() => addChainEntry(c.index, addScene[c.index] ?? 0)}>+ add scene</button>
              </div>
            {/if}
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
    margin-bottom: 8px;
  }
  h2 {
    font-size: 12px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--ember);
    margin: 0;
  }
  .list {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .scene {
    display: flex;
    align-items: center;
    gap: 5px;
    border: var(--border);
    border-radius: var(--radius);
    padding: 5px 7px;
    background: var(--panel);
  }
  .idx {
    color: var(--dim);
    min-width: 16px;
  }
  .nm {
    flex: 1;
    color: var(--fg);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .nmbtn {
    text-align: left;
    background: transparent;
    border: 1px solid transparent;
  }
  .nmbtn:hover {
    border-color: var(--dim);
  }
  .ren {
    flex: 1;
    font-size: 12px;
  }
  .mini {
    padding: 2px 6px;
    font-size: 11px;
    color: var(--fg-dim);
    background: transparent;
  }
  .mini.on {
    color: var(--aqua);
    border-color: var(--aqua);
  }
  .mini.play {
    color: var(--ok);
  }
  .mini.stop {
    color: var(--err);
  }
  .mini.del {
    color: var(--err);
  }
  .chain {
    border: var(--border);
    border-left: 3px solid var(--dim);
    border-radius: var(--radius);
    padding: 7px;
    background: var(--panel);
  }
  .chain.playing {
    border-left-color: var(--ok);
  }
  .chain-head {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .entries {
    display: flex;
    flex-direction: column;
    gap: 3px;
    margin: 6px 0;
  }
  .entry {
    display: flex;
    align-items: center;
    gap: 3px;
    font-size: 11px;
  }
  .entry.cur {
    background: var(--panel-2);
    border-radius: 2px;
  }
  .escene {
    flex: 1;
    text-align: left;
    background: transparent;
    border: none;
    color: var(--fg);
  }
  .entry.cur .escene {
    color: var(--ok);
    font-weight: 700;
  }
  .ekv {
    color: var(--fg-dim);
    min-width: 26px;
    text-align: right;
  }
  .addentry {
    display: flex;
    gap: 4px;
    margin-top: 4px;
  }
  .addentry select {
    flex: 1;
    background: var(--bg);
    color: var(--fg);
    border: var(--border);
    border-radius: var(--radius);
    font-family: inherit;
    font-size: 11px;
    padding: 2px 4px;
  }
  .small {
    font-size: 11px;
  }
</style>
