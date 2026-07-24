<script lang="ts">
  import { applyChordProgression } from "../lib/store.svelte";

  // `lane` is the target CHORDS lane; `close` dismisses the modal.
  let { lane, close }: { lane: number; close: () => void } = $props();

  let text = $state("");
  let error = $state<string | null>(null);
  let busy = $state(false);
  let input = $state<HTMLInputElement | null>(null);

  const examples = [
    "Dm7 G7 Cmaj7",
    "Am F C G",
    "Cmaj9 Am9 Dm9 G9",
    "Em C G D",
  ];

  $effect(() => {
    input?.focus();
  });

  async function apply() {
    const t = text.trim();
    if (!t || busy) return;
    busy = true;
    error = await applyChordProgression(lane, t);
    busy = false;
    if (!error) close();
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "Enter") apply();
    else if (e.key === "Escape") close();
  }
</script>

<div
  class="backdrop"
  role="button"
  tabindex="-1"
  aria-label="Close"
  onclick={close}
  onkeydown={(e) => e.key === "Escape" && close()}
></div>

<div class="modal" role="dialog" aria-modal="true" aria-label="Type a chord progression">
  <h2>Chord progression <span class="arrow">→ CHORDS</span></h2>
  <p class="hint">
    Type chord names separated by spaces or commas. One sustained chord per bar,
    voiced to four notes for the J-6 with smooth voice-leading.
  </p>

  <input
    bind:this={input}
    bind:value={text}
    class="field mono"
    placeholder="e.g. Dm7 G7 Cmaj7 A7"
    onkeydown={onKey}
    spellcheck="false"
    autocapitalize="off"
    autocomplete="off"
  />

  {#if error}
    <p class="err">{error}</p>
  {/if}

  <div class="examples">
    {#each examples as ex (ex)}
      <button class="chip mono" onclick={() => (text = ex)}>{ex}</button>
    {/each}
  </div>

  <p class="vocab">
    Qualities: maj · m · 7 · maj7 · m7 · m7b5 · dim · dim7 · aug · 6 · m6 · 9 ·
    maj9 · m9 · add9 · sus2 · sus4 · 7sus4. Accidentals <span class="mono">#</span>/<span class="mono">b</span>.
  </p>

  <div class="actions">
    <button class="cancel" onclick={close}>Cancel</button>
    <button class="apply" disabled={!text.trim() || busy} onclick={apply}>
      {busy ? "…" : "Apply"}
    </button>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.55);
    z-index: 40;
    border: none;
  }
  .modal {
    position: fixed;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    z-index: 41;
    width: min(560px, 92vw);
    background: var(--panel);
    border: var(--border-strong);
    border-radius: 6px;
    box-shadow: 0 12px 48px rgba(0, 0, 0, 0.5);
    padding: 18px 20px;
  }
  h2 {
    margin: 0 0 8px;
    font-size: 14px;
    color: var(--fg);
  }
  .arrow {
    color: var(--green);
    font-weight: 700;
    font-size: 12px;
    letter-spacing: 0.06em;
  }
  .hint {
    margin: 0 0 12px;
    font-size: 11px;
    color: var(--fg-dim);
    line-height: 1.5;
  }
  .field {
    width: 100%;
    box-sizing: border-box;
    font-size: 15px;
    padding: 8px 10px;
    background: var(--bg);
    color: var(--fg);
    border: 1px solid var(--dim);
    border-radius: 4px;
  }
  .field:focus {
    outline: none;
    border-color: var(--green);
  }
  .err {
    margin: 8px 0 0;
    font-size: 11px;
    color: var(--err);
  }
  .examples {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin: 12px 0 0;
  }
  .chip {
    font-size: 11px;
    padding: 2px 8px;
    border-radius: 999px;
    background: var(--panel-2);
    color: var(--fg-dim);
    border: 1px solid var(--dim);
    cursor: pointer;
  }
  .chip:hover {
    color: var(--fg);
    border-color: var(--green);
  }
  .vocab {
    margin: 12px 0 0;
    font-size: 10px;
    color: var(--dim);
    line-height: 1.6;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 16px;
  }
  .actions button {
    padding: 5px 14px;
    font-size: 12px;
  }
  .apply {
    color: var(--bg);
    background: var(--green);
    border-color: var(--green);
    font-weight: 700;
  }
  .apply:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .cancel {
    color: var(--fg-dim);
  }
</style>
