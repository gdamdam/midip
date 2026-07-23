<script lang="ts">
  import { app, loadPattern, audition, endAudition, favorite, send, userPatternCmd, crateAdd } from "../lib/store.svelte";
  import { libraryQuery } from "../lib/bridge";
  import type { LibRecord, LibQuery } from "../lib/types";
  import CratesPanel from "./CratesPanel.svelte";

  let role = $state("drums");
  let query = $state("");
  let favOnly = $state(false);
  // Phase 8 facet filters ("" = any).
  let feel = $state("");
  let func = $state("");
  let poly = $state("");
  let auditioning = $state<string | null>(null);
  let confirmDelete = $state<string | null>(null);
  let showUser = $state(false);
  let crateTarget = $state<number | null>(null);
  const crates = $derived(app.snap?.crates ?? []);

  const roleData = $derived(app.library?.roles.find((r) => r.role === role) ?? null);

  // Active whenever text or any facet narrows the browse — then we defer to the
  // shared Rust engine (one source of truth for matching + ordering) instead of
  // the local genre view.
  const queryActive = $derived(
    query.trim().length > 0 || favOnly || feel !== "" || func !== "" || poly !== "",
  );

  // Filtered records from the shared engine (only fetched while queryActive).
  let results = $state<LibRecord[]>([]);
  let reqToken = 0;
  $effect(() => {
    // track deps
    const q: LibQuery = {
      text: query,
      role,
      feel: feel || null,
      function: func || null,
      poly: poly || null,
      favorites_only: favOnly,
    };
    if (!queryActive) {
      results = [];
      return;
    }
    const token = ++reqToken;
    libraryQuery(q).then((rows) => {
      if (token === reqToken) results = rows; // ignore stale responses
    });
  });

  // Present both modes through one genre-grouped shape the template renders.
  const filtered = $derived.by(() => {
    if (queryActive) {
      const groups = new Map<string, LibRecord[]>();
      for (const r of results) {
        if (!groups.has(r.genre)) groups.set(r.genre, []);
        groups.get(r.genre)!.push(r);
      }
      return [...groups.entries()].map(([name, patterns]) => ({ name, patterns }));
    }
    if (!roleData) return [];
    return roleData.genres
      .map((g) => ({ name: g.name, patterns: g.patterns }))
      .filter((g) => g.patterns.length > 0);
  });

  const activeChips = $derived(
    [
      query.trim() && `“${query.trim()}”`,
      favOnly && "★ favorites",
      feel && `feel:${feel}`,
      func && `fn:${func}`,
      poly && `${poly}`,
    ].filter(Boolean) as string[],
  );

  function clearFilters() {
    query = "";
    favOnly = false;
    feel = "";
    func = "";
    poly = "";
  }

  async function doAudition(genre: string, name: string) {
    auditioning = `${genre}/${name}`;
    await audition(role, genre, name);
  }
  async function doStop() {
    auditioning = null;
    await endAudition();
  }
</script>

<section class="library">
  <div class="head">
    <div class="roles">
      {#each ["drums", "bass", "synth"] as r (r)}
        <button class="role" class:active={role === r} onclick={() => (role = r)}>{r}</button>
      {/each}
    </div>
    <div class="filters">
      <input class="search" placeholder="search name, desc, tags…" bind:value={query} />
      <button class="fav-filter" class:on={favOnly} onclick={() => (favOnly = !favOnly)} title="Favorites only">
        ★
      </button>
    </div>
    <div class="facets">
      <select bind:value={feel} title="Feel">
        <option value="">feel: any</option>
        <option value="straight">straight</option>
        <option value="swing">swing</option>
        <option value="triplet">triplet</option>
      </select>
      <select bind:value={func} title="Function">
        <option value="">function: any</option>
        <option value="core">core</option>
        <option value="variation_a">variation A</option>
        <option value="variation_b">variation B</option>
        <option value="fill">fill</option>
        <option value="breakdown">breakdown</option>
        <option value="peak">peak</option>
      </select>
      <select bind:value={poly} title="Chord / mono">
        <option value="">voicing: any</option>
        <option value="mono">mono</option>
        <option value="poly">chord</option>
      </select>
    </div>
    {#if activeChips.length > 0}
      <div class="active-filters">
        {#each activeChips as chip (chip)}<span class="chip">{chip}</span>{/each}
        <button class="clear" onclick={clearFilters} title="Clear all filters">clear ✕</button>
      </div>
    {/if}
    {#if crates.length > 0}
      <div class="crate-target">
        <span class="muted small">＋ to crate:</span>
        <select bind:value={crateTarget}>
          <option value={null}>— off —</option>
          {#each crates as c (c.index)}
            <option value={c.index}>{c.name}</option>
          {/each}
        </select>
      </div>
    {/if}
  </div>

  {#if auditioning}
    <div class="audition-bar">
      <span class="mono">♪ auditioning {auditioning}</span>
      <button onclick={doStop}>Stop</button>
    </div>
  {/if}

  {#if !app.library}
    <p class="muted pad">Loading library…</p>
  {:else if filtered.length === 0}
    <p class="muted pad">No patterns{favOnly ? " favorited" : query ? " match your filter" : " in this role"}.</p>
  {:else}
    <div class="list">
      {#each filtered as genre (genre.name)}
        <div class="genre">
          <div class="gname">{genre.name}</div>
          {#each genre.patterns as p (p.name)}
            <div class="pat">
              <button
                class="star"
                class:on={p.favorite}
                onclick={() => favorite(role, genre.name, p.name)}
                aria-label={p.favorite ? "Unfavorite" : "Favorite"}
              >{p.favorite ? "★" : "☆"}</button>
              <button
                class="load"
                onclick={() => loadPattern(role, genre.name, p.name)}
                title="Load / queue into the {role} lane"
              >
                <span class="pn">{p.name}</span>
                {#if p.function}
                  <span class="fam" title="Family: {p.family}">{p.function}</span>
                {/if}
                <span class="meta mono">{p.length}</span>
              </button>
              <button
                class="aud"
                onclick={() => doAudition(genre.name, p.name)}
                title="Audition (preview without committing)"
                aria-label="Audition {p.name}"
              >♪</button>
              {#if crateTarget !== null}
                <button class="aud" onclick={() => crateAdd(crateTarget!, role, genre.name, p.name)} title="Add to selected crate">＋</button>
              {/if}
            </div>
          {/each}
        </div>
      {/each}
    </div>
  {/if}
  <div class="mine">
    <button class="mine-head" onclick={() => (showUser = !showUser)}>
      {showUser ? "▾" : "▸"} My patterns ({app.userPatterns.length})
    </button>
    {#if showUser}
      <div class="mine-list">
        {#if app.userPatterns.length === 0}
          <span class="muted small">Save a lane's pattern with “save→lib”.</span>
        {:else}
          {#each app.userPatterns as up (up.path)}
            <div class="mine-row">
              <button class="mine-load" onclick={() => send({ type: "loadUserPattern", args: up.path })} title="Load into its role's lane">
                <span class="pn">{up.name}</span><span class="meta mono">{up.kind[0]}·{up.length}</span>
              </button>
              <button class="mini" onclick={() => userPatternCmd({ type: "duplicateUserPattern", args: up.path })} title="Duplicate">⧉</button>
              {#if confirmDelete === up.path}
                <button class="mini del" onclick={() => { userPatternCmd({ type: "deleteUserPattern", args: up.path }); confirmDelete = null; }}>sure?</button>
              {:else}
                <button class="mini" onclick={() => (confirmDelete = up.path)} title="Delete">🗑</button>
              {/if}
            </div>
          {/each}
        {/if}
      </div>
    {/if}
  </div>

  <CratesPanel />

  <p class="hint muted">Load queues at the next bar while playing · ♪ auditions on a muted/stopped lane.</p>
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
  .filters {
    display: flex;
    gap: 6px;
  }
  .facets {
    display: flex;
    gap: 6px;
    margin-top: 6px;
  }
  .facets select {
    flex: 1;
    font-size: 11px;
  }
  .active-filters {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px;
    margin-top: 6px;
  }
  .chip {
    font-size: 10px;
    padding: 1px 6px;
    border-radius: 999px;
    background: var(--panel-2);
    color: var(--fg);
  }
  .clear {
    font-size: 10px;
    color: var(--dim);
    background: transparent;
    border: none;
    cursor: pointer;
  }
  .search {
    flex: 1;
  }
  .fav-filter {
    color: var(--dim);
    min-width: 32px;
  }
  .fav-filter.on {
    color: var(--warn);
    border-color: var(--warn);
  }
  .crate-target {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .crate-target select {
    flex: 1;
    background: var(--bg);
    color: var(--fg);
    border: var(--border);
    border-radius: var(--radius);
    font-family: inherit;
    font-size: 11px;
    padding: 2px 4px;
  }
  .audition-bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 5px 8px;
    background: var(--panel-2);
    border-bottom: var(--border);
    color: var(--pink);
    font-size: 11px;
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
    align-items: center;
    gap: 2px;
    width: 100%;
    border: 1px solid transparent;
    border-radius: var(--radius);
  }
  .pat:hover {
    background: var(--panel-2);
    border-color: var(--dim);
  }
  .star,
  .aud {
    background: transparent;
    border: none;
    color: var(--dim);
    padding: 4px 6px;
  }
  .star.on {
    color: var(--warn);
  }
  .star:hover,
  .aud:hover {
    color: var(--fg);
  }
  .load {
    flex: 1;
    display: flex;
    justify-content: space-between;
    text-align: left;
    background: transparent;
    border: none;
    padding: 4px 4px;
  }
  .pn {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .fam {
    color: var(--ember);
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    padding: 0 4px;
    align-self: center;
    white-space: nowrap;
  }
  .meta {
    color: var(--dim);
    font-size: 11px;
  }
  .pad {
    padding: 12px;
  }
  .mine {
    border-top: var(--border);
    padding: 6px;
    max-height: 30%;
    overflow-y: auto;
  }
  .mine-head {
    width: 100%;
    text-align: left;
    background: transparent;
    border: none;
    color: var(--pink);
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }
  .mine-list {
    display: flex;
    flex-direction: column;
    gap: 3px;
    margin-top: 4px;
  }
  .mine-row {
    display: flex;
    align-items: center;
    gap: 3px;
  }
  .mine-load {
    flex: 1;
    display: flex;
    justify-content: space-between;
    text-align: left;
    background: transparent;
    border: 1px solid transparent;
  }
  .mine-load:hover {
    background: var(--panel-2);
    border-color: var(--dim);
  }
  .mini {
    padding: 2px 6px;
    background: transparent;
    color: var(--fg-dim);
  }
  .mini.del {
    color: var(--err);
    border-color: var(--err);
    font-size: 10px;
  }
  .hint {
    font-size: 10px;
    padding: 6px 8px;
    border-top: var(--border);
  }
</style>
