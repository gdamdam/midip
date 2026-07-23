<script lang="ts">
  import { app, loadPattern } from "../lib/store.svelte";

  let role = $state("drums");
  let query = $state("");

  const roleData = $derived(app.library?.roles.find((r) => r.role === role) ?? null);

  const filtered = $derived.by(() => {
    if (!roleData) return [];
    const q = query.trim().toLowerCase();
    return roleData.genres
      .map((g) => ({
        name: g.name,
        patterns: q
          ? g.patterns.filter(
              (p) => p.name.toLowerCase().includes(q) || g.name.toLowerCase().includes(q),
            )
          : g.patterns,
      }))
      .filter((g) => g.patterns.length > 0);
  });
</script>

<section class="library">
  <div class="head">
    <div class="roles">
      {#each ["drums", "bass", "synth"] as r (r)}
        <button class="role" class:active={role === r} onclick={() => (role = r)}>{r}</button>
      {/each}
    </div>
    <input class="search" placeholder="filter patterns…" bind:value={query} />
  </div>

  {#if !app.library}
    <p class="muted pad">Loading library…</p>
  {:else if filtered.length === 0}
    <p class="muted pad">No patterns{query ? " match your filter" : " in this role"}.</p>
  {:else}
    <div class="list">
      {#each filtered as genre (genre.name)}
        <div class="genre">
          <div class="gname">{genre.name}</div>
          {#each genre.patterns as p (p.name)}
            <button
              class="pat"
              onclick={() => loadPattern(role, genre.name, p.name)}
              title="Load / queue into the {role} lane"
            >
              <span class="pn">{p.name}</span>
              <span class="meta mono">{p.length}</span>
            </button>
          {/each}
        </div>
      {/each}
    </div>
  {/if}
  <p class="hint muted">Loads into the role's lane — queued at the next bar while playing.</p>
</section>

<style>
  .library {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
  }
  .head {
    padding: 8px;
    border-bottom: var(--border);
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .roles {
    display: flex;
    gap: 4px;
  }
  .role {
    flex: 1;
    text-transform: capitalize;
    color: var(--fg-dim);
  }
  .role.active {
    color: var(--fg);
    border-color: var(--fg-dim);
    background: var(--panel-2);
  }
  .search {
    width: 100%;
  }
  .list {
    overflow-y: auto;
    flex: 1;
    padding: 6px;
  }
  .genre {
    margin-bottom: 10px;
  }
  .gname {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--ember);
    padding: 2px 4px;
    position: sticky;
    top: 0;
    background: var(--bg);
  }
  .pat {
    display: flex;
    justify-content: space-between;
    width: 100%;
    text-align: left;
    background: transparent;
    border: 1px solid transparent;
    padding: 4px 6px;
    border-radius: var(--radius);
  }
  .pat:hover {
    background: var(--panel-2);
    border-color: var(--dim);
  }
  .pn {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meta {
    color: var(--dim);
    font-size: 11px;
  }
  .pad {
    padding: 12px;
  }
  .hint {
    font-size: 10px;
    padding: 6px 8px;
    border-top: var(--border);
  }
</style>
