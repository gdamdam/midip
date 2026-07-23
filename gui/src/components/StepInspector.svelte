<script lang="ts">
  import { app, send } from "../lib/store.svelte";
  import { midiName, condLabel } from "../lib/music";
  import type { GuiCommand } from "../lib/types";

  const insp = $derived(app.snap!.inspector);
  const sel = $derived(app.snap!.selection);
  const lane = $derived(app.snap!.focused_lane);

  // Cell-targeted commands all carry the current selection.
  function cell(
    type:
      | "adjustVel"
      | "adjustProb"
      | "adjustRatchet"
      | "adjustMicro"
      | "adjustLen"
      | "adjustCcVal",
    delta: number,
  ): GuiCommand {
    return { type, args: { lane, row: sel.row, col: sel.col, delta } } as GuiCommand;
  }
  function bare(
    type: "cycleCond" | "toggleSlide" | "clearStep" | "copyStep" | "pasteStep" | "ccAdd" | "ccRemove",
  ): GuiCommand {
    return { type, args: { lane, row: sel.row, col: sel.col } } as GuiCommand;
  }
</script>

<aside class="inspector">
  <div class="ihead">
    <h2>Step inspector</h2>
    <span class="stepno mono">step {sel.col + 1}</span>
  </div>

  {#if !insp.present}
    <p class="empty muted">
      Nothing on step {sel.col + 1}. Click a cell in the grid to place a step, then edit its
      velocity, probability, ratchet, length, microtiming, trig condition and CC here.
    </p>
  {:else}
    {#if insp.pitch !== null && insp.kind === "melodic"}
      <div class="param">
        <span class="k">Pitch</span>
        <button onclick={() => send({ type: "noteDown", args: { lane, col: sel.col } })}>−</button>
        <span class="v mono">{midiName(insp.pitch)}</span>
        <button onclick={() => send({ type: "noteUp", args: { lane, col: sel.col } })}>+</button>
      </div>
    {/if}

    {#if insp.velocity !== null}
      <div class="param">
        <span class="k">Velocity</span>
        <button onclick={() => send(cell("adjustVel", -1))}>−</button>
        <span class="v mono">{insp.velocity}</span>
        <button onclick={() => send(cell("adjustVel", 1))}>+</button>
      </div>
    {/if}
    {#if insp.vel_mult !== null}
      <div class="param">
        <span class="k">Velocity</span>
        <button onclick={() => send(cell("adjustVel", -1))}>−</button>
        <span class="v mono">×{insp.vel_mult.toFixed(2)}</span>
        <button onclick={() => send(cell("adjustVel", 1))}>+</button>
      </div>
    {/if}

    {#if insp.probability !== null}
      <div class="param">
        <span class="k">Probability</span>
        <button onclick={() => send(cell("adjustProb", -1))}>−</button>
        <span class="v mono">{Math.round(insp.probability * 100)}%</span>
        <button onclick={() => send(cell("adjustProb", 1))}>+</button>
      </div>
    {/if}

    {#if insp.ratchet !== null}
      <div class="param">
        <span class="k">Ratchet</span>
        <button onclick={() => send(cell("adjustRatchet", -1))}>−</button>
        <span class="v mono">×{insp.ratchet}</span>
        <button onclick={() => send(cell("adjustRatchet", 1))}>+</button>
      </div>
    {/if}

    {#if insp.length !== null}
      <div class="param">
        <span class="k">Length</span>
        <button onclick={() => send(cell("adjustLen", -1))}>−</button>
        <span class="v mono">{insp.length.toFixed(2)}</span>
        <button onclick={() => send(cell("adjustLen", 1))}>+</button>
      </div>
    {/if}

    {#if insp.slide !== null}
      <div class="param">
        <span class="k">Slide</span>
        <button class="toggle" class:on={insp.slide} onclick={() => send(bare("toggleSlide"))}>
          {insp.slide ? "ON" : "off"}
        </button>
      </div>
    {/if}

    {#if insp.micro !== null}
      <div class="param">
        <span class="k">Microtiming</span>
        <button onclick={() => send(cell("adjustMicro", -1))}>−</button>
        <span class="v mono">{insp.micro > 0 ? "+" : ""}{insp.micro}</span>
        <button onclick={() => send(cell("adjustMicro", 1))}>+</button>
      </div>
    {/if}

    <div class="param">
      <span class="k">Trig cond</span>
      <button class="wide" onclick={() => send(bare("cycleCond"))}>{condLabel(insp.cond)}</button>
    </div>

    <div class="cc">
      <div class="cc-head">
        <span class="k">CC locks</span>
        <button onclick={() => send(bare("ccAdd"))} title="Add CC lock">+</button>
        <button onclick={() => send(bare("ccRemove"))} title="Remove last CC lock">−</button>
      </div>
      {#if insp.cc.length === 0}
        <span class="muted small">none</span>
      {:else}
        {#each insp.cc as c (c.cc)}
          <div class="ccrow mono">
            <span>CC{c.cc}</span>
            <button onclick={() => send(cell("adjustCcVal", -1))}>−</button>
            <span class="v">{c.val}</span>
            <button onclick={() => send(cell("adjustCcVal", 1))}>+</button>
          </div>
        {/each}
      {/if}
    </div>

    <div class="clip">
      <button onclick={() => send(bare("copyStep"))}>Copy</button>
      <button onclick={() => send(bare("pasteStep"))}>Paste</button>
      <button class="danger" onclick={() => send(bare("clearStep"))}>Clear</button>
    </div>
  {/if}
</aside>

<style>
  .inspector {
    padding: 10px 12px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    overflow-y: auto;
    height: 100%;
  }
  .ihead {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 8px;
    margin-bottom: 4px;
  }
  h2 {
    margin: 0;
    font-size: 12px;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--ember);
  }
  .stepno {
    font-size: 11px;
    color: var(--fg-dim);
  }
  .empty {
    font-size: 12px;
    line-height: 1.5;
  }
  .param {
    display: grid;
    grid-template-columns: 1fr auto auto auto;
    align-items: center;
    gap: 6px;
  }
  .param .k {
    font-size: 11px;
    color: var(--fg-dim);
  }
  .param .v {
    min-width: 46px;
    text-align: center;
  }
  .param button {
    padding: 2px 8px;
    min-width: 26px;
  }
  .param .wide {
    grid-column: 2 / 5;
  }
  .toggle.on {
    color: var(--ok);
    border-color: var(--ok);
  }
  .cc {
    border-top: var(--border);
    padding-top: 8px;
  }
  .cc-head {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .cc-head .k {
    flex: 1;
    font-size: 11px;
    color: var(--fg-dim);
  }
  .ccrow {
    display: grid;
    grid-template-columns: 1fr auto auto auto;
    gap: 6px;
    align-items: center;
    margin-top: 4px;
  }
  .small {
    font-size: 11px;
  }
  .clip {
    display: flex;
    gap: 6px;
    border-top: var(--border);
    padding-top: 8px;
    margin-top: 2px;
  }
  .clip button {
    flex: 1;
  }
  .danger {
    color: var(--err);
  }
</style>
