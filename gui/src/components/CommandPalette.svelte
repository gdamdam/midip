<script lang="ts">
  interface Cmd {
    label: string;
    hint?: string;
    run: () => void;
  }
  interface Props {
    actions: Cmd[];
    close: () => void;
  }
  let { actions, close }: Props = $props();

  let query = $state("");
  let sel = $state(0);

  const filtered = $derived.by(() => {
    const q = query.trim().toLowerCase();
    const list = q
      ? actions.filter((a) => a.label.toLowerCase().includes(q))
      : actions;
    return list;
  });

  $effect(() => {
    // Keep the selection in range as the filter narrows.
    if (sel >= filtered.length) sel = Math.max(0, filtered.length - 1);
  });

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      close();
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      sel = Math.min(filtered.length - 1, sel + 1);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      sel = Math.max(0, sel - 1);
    } else if (e.key === "Enter") {
      e.preventDefault();
      const a = filtered[sel];
      if (a) {
        a.run();
        close();
      }
    }
  }
  function focus(node: HTMLInputElement) {
    node.focus();
  }
</script>

<div class="scrim" role="presentation" onclick={close}></div>
<div class="palette" role="dialog" aria-label="Command palette">
  <input
    class="q"
    placeholder="Type a command…"
    bind:value={query}
    onkeydown={onKey}
    use:focus
  />
  <div class="results">
    {#each filtered as a, i (a.label)}
      <button
        class="item"
        class:sel={i === sel}
        onmouseenter={() => (sel = i)}
        onclick={() => {
          a.run();
          close();
        }}
      >
        <span class="lbl">{a.label}</span>
        {#if a.hint}<span class="hint mono">{a.hint}</span>{/if}
      </button>
    {/each}
    {#if filtered.length === 0}
      <div class="empty muted">no match</div>
    {/if}
  </div>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.4);
    z-index: 70;
  }
  .palette {
    position: fixed;
    top: 12%;
    left: 50%;
    transform: translateX(-50%);
    width: 460px;
    max-width: 92vw;
    max-height: 60vh;
    display: flex;
    flex-direction: column;
    background: var(--panel);
    border: var(--border-strong);
    border-radius: 6px;
    box-shadow: 0 16px 48px rgba(0, 0, 0, 0.55);
    z-index: 71;
    overflow: hidden;
  }
  .q {
    border: none;
    border-bottom: var(--border);
    border-radius: 0;
    padding: 12px 14px;
    font-size: 15px;
    background: var(--bg);
  }
  .results {
    overflow-y: auto;
  }
  .item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    width: 100%;
    text-align: left;
    background: transparent;
    border: none;
    border-radius: 0;
    padding: 7px 14px;
  }
  .item.sel {
    background: var(--selection);
  }
  .lbl {
    color: var(--fg);
  }
  .hint {
    color: var(--dim);
    font-size: 11px;
  }
  .empty {
    padding: 14px;
    font-size: 12px;
  }
</style>
