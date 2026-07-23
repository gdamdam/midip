<script lang="ts">
  import {
    app,
    crateCreate,
    crateRename,
    crateDelete,
    crateRemoveEntry,
    crateMoveEntry,
    crateLaunch,
  } from "../lib/store.svelte";

  const crates = $derived(app.snap!.crates);

  let open = $state(false);
  let newName = $state("");
  let ren = $state<number | null>(null);
  let renName = $state("");
  let confirmDel = $state<number | null>(null);

  function create() {
    const n = newName.trim();
    if (!n) return;
    crateCreate(n);
    newName = "";
  }
  function commitRen() {
    if (ren !== null && renName.trim()) crateRename(ren, renName.trim());
    ren = null;
  }
</script>

<div class="crates">
  <button class="head" onclick={() => (open = !open)}>
    {open ? "▾" : "▸"} Crates ({crates.length})
  </button>
  {#if open}
    <div class="new">
      <input placeholder="new crate name" bind:value={newName}
        onkeydown={(e) => e.key === "Enter" && create()} />
      <button onclick={create}>+</button>
    </div>
    {#each crates as c (c.index)}
      <div class="crate">
        <div class="crate-head">
          {#if ren === c.index}
            <input class="ren" bind:value={renName}
              onkeydown={(e) => e.key === "Enter" && commitRen()} onblur={commitRen} />
          {:else}
            <button class="nm" onclick={() => { ren = c.index; renName = c.name; }} title="Rename">{c.name}</button>
          {/if}
          {#if confirmDel === c.index}
            <button class="mini del" onclick={() => { crateDelete(c.index); confirmDel = null; }}>sure?</button>
          {:else}
            <button class="mini" onclick={() => (confirmDel = c.index)} title="Delete crate">🗑</button>
          {/if}
        </div>
        {#if c.entries.length === 0}
          <span class="muted tiny">empty — add patterns from the browser (＋)</span>
        {:else}
          {#each c.entries as e, ei (ei)}
            <div class="cent">
              <button class="clabel" onclick={() => crateLaunch(c.index, ei)} title="Load / queue">{e.label}</button>
              <button class="mini" onclick={() => crateMoveEntry(c.index, ei, Math.max(0, ei - 1))} title="Up">↑</button>
              <button class="mini" onclick={() => crateMoveEntry(c.index, ei, ei + 1)} title="Down">↓</button>
              <button class="mini del" onclick={() => crateRemoveEntry(c.index, ei)}>✕</button>
            </div>
          {/each}
        {/if}
      </div>
    {/each}
  {/if}
</div>

<style>
  .crates {
    border-top: var(--border);
    padding: 6px;
    max-height: 32%;
    overflow-y: auto;
  }
  .head {
    width: 100%;
    text-align: left;
    background: transparent;
    border: none;
    color: var(--aqua);
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }
  .new {
    display: flex;
    gap: 4px;
    margin: 4px 0;
  }
  .new input {
    flex: 1;
    font-size: 11px;
  }
  .crate {
    border: var(--border);
    border-radius: var(--radius);
    padding: 5px;
    margin-bottom: 5px;
    background: var(--panel);
  }
  .crate-head {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .nm {
    flex: 1;
    text-align: left;
    background: transparent;
    border: 1px solid transparent;
    color: var(--fg);
    font-size: 12px;
  }
  .nm:hover {
    border-color: var(--dim);
  }
  .ren {
    flex: 1;
    font-size: 12px;
  }
  .cent {
    display: flex;
    align-items: center;
    gap: 3px;
    margin-top: 3px;
  }
  .clabel {
    flex: 1;
    text-align: left;
    background: transparent;
    border: 1px solid transparent;
    font-size: 11px;
  }
  .clabel:hover {
    background: var(--panel-2);
    border-color: var(--dim);
  }
  .mini {
    padding: 2px 5px;
    font-size: 11px;
    color: var(--fg-dim);
    background: transparent;
  }
  .mini.del {
    color: var(--err);
  }
  .tiny {
    font-size: 10px;
    padding: 2px 4px;
  }
</style>
