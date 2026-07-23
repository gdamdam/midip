<script lang="ts">
  import { app, send, refreshSets } from "../lib/store.svelte";

  const t = $derived(app.snap!.transport);
  const lanes = $derived(app.snap!.lanes);

  let saveAsName = $state("");
  let showSaveAs = $state(false);

  async function doSave() {
    await send({ type: "save" });
    await refreshSets();
  }
  async function doSaveAs() {
    const n = saveAsName.trim();
    if (!n) return;
    await send({ type: "saveSetAs", args: n });
    showSaveAs = false;
    saveAsName = "";
    await refreshSets();
  }
  async function loadSet(path: string) {
    await send({ type: "loadSet", args: path });
  }
  async function newSet() {
    await send({ type: "newSet" });
  }
</script>

<section class="setup">
  <div class="col">
    <h2>Set</h2>
    <div class="row">
      <span class="name">{t.set_name || "untitled"}</span>
      <span class="state" class:edited={t.dirty}>{t.dirty ? "EDITED" : "SAVED"}</span>
    </div>
    <div class="actions">
      <button onclick={doSave}>Save</button>
      <button onclick={() => (showSaveAs = !showSaveAs)}>Save As…</button>
      <button onclick={newSet}>New</button>
    </div>
    {#if showSaveAs}
      <div class="saveas">
        <input placeholder="new set name" bind:value={saveAsName}
          onkeydown={(e) => e.key === "Enter" && doSaveAs()} />
        <button onclick={doSaveAs}>OK</button>
      </div>
    {/if}

    <h3>Saved sets</h3>
    <div class="sets">
      {#if app.sets.length === 0}
        <span class="muted small">none yet</span>
      {:else}
        {#each app.sets as s (s.path)}
          <button class="setrow" onclick={() => loadSet(s.path)}>{s.name}</button>
        {/each}
      {/if}
    </div>
  </div>

  <div class="col">
    <h2>Routing</h2>
    <div class="routes">
      {#each lanes as l (l.index)}
        <div class="route">
          <div class="rhead">
            <span class="lb">{l.label}</span>
            <span class="conn" class:ok={l.connected} title={l.connected ? "connected" : "not found"}>
              {l.connected ? "●" : "○"}
            </span>
          </div>
          <div class="rctl">
            <span class="rk">Port</span>
            <button onclick={() => send({ type: "cycleRoutePort", args: { lane: l.index, delta: -1 } })} aria-label="Previous port">‹</button>
            <span class="port" class:def={l.route_default} title={l.route_port}>
              {l.route_default ? `${l.route_port} (default)` : l.route_port}
            </span>
            <button onclick={() => send({ type: "cycleRoutePort", args: { lane: l.index, delta: 1 } })} aria-label="Next port">›</button>
          </div>
          <div class="rctl">
            <span class="rk">Channel</span>
            <button onclick={() => send({ type: "adjustRouteChannel", args: { lane: l.index, delta: -1 } })} aria-label="Channel down">−</button>
            <span class="v mono">{l.channel}</span>
            <button onclick={() => send({ type: "adjustRouteChannel", args: { lane: l.index, delta: 1 } })} aria-label="Channel up">+</button>
            <span class="rk clk">Clock</span>
            <button class="toggle" class:on={l.clock_out} onclick={() => send({ type: "toggleClockOut", args: l.index })}>
              {l.clock_out ? "ON" : "off"}
            </button>
          </div>
        </div>
      {/each}
    </div>

    <div class="row toggles">
      <span>Virtual-port mirror</span>
      <button class="toggle" class:on={t.mirror} onclick={() => send({ type: "toggleMirror" })}>
        {t.mirror ? "ON" : "off"}
      </button>
    </div>
  </div>
</section>

<style>
  .setup {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 20px;
    padding: 16px;
    overflow-y: auto;
    height: 100%;
  }
  .col {
    min-width: 0;
  }
  h2 {
    font-size: 12px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--ember);
    margin: 0 0 8px;
  }
  h3 {
    font-size: 11px;
    text-transform: uppercase;
    color: var(--fg-dim);
    margin: 16px 0 6px;
  }
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    margin-bottom: 8px;
  }
  .name {
    font-weight: 700;
  }
  .state {
    font-size: 11px;
    color: var(--ok);
  }
  .state.edited {
    color: var(--warn);
  }
  .actions {
    display: flex;
    gap: 6px;
  }
  .saveas {
    display: flex;
    gap: 6px;
    margin-top: 8px;
  }
  .saveas input {
    flex: 1;
  }
  .sets {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .setrow {
    text-align: left;
    background: transparent;
  }
  .setrow:hover {
    background: var(--panel-2);
  }
  .routes {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .route {
    border: var(--border);
    border-radius: var(--radius);
    padding: 8px;
    background: var(--panel);
  }
  .rhead {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 6px;
  }
  .rctl {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-top: 4px;
  }
  .rk {
    font-size: 10px;
    text-transform: uppercase;
    color: var(--fg-dim);
    min-width: 48px;
  }
  .rk.clk {
    min-width: 0;
    margin-left: 10px;
  }
  .rctl button {
    padding: 2px 8px;
    min-width: 24px;
  }
  .port {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 12px;
    color: var(--aqua);
  }
  .port.def {
    color: var(--fg-dim);
  }
  .v {
    min-width: 26px;
    text-align: center;
  }
  .lb {
    color: var(--fg);
    font-weight: 700;
  }
  .conn {
    color: var(--dim);
  }
  .conn.ok {
    color: var(--aqua);
  }
  .toggles {
    margin-top: 14px;
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .toggle.on {
    color: var(--ok);
    border-color: var(--ok);
  }
  .small {
    font-size: 11px;
  }
</style>
