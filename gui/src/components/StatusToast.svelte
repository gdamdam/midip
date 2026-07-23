<script lang="ts">
  import { app } from "../lib/store.svelte";

  let visible = $state(false);
  let text = $state("");

  // Show the status line briefly whenever it changes.
  let last = "";
  $effect(() => {
    const s = app.snap?.status ?? "";
    if (s && s !== last) {
      last = s;
      text = s;
      visible = true;
      const id = setTimeout(() => (visible = false), 2600);
      return () => clearTimeout(id);
    }
  });
</script>

{#if visible && text}
  <div class="toast" role="status">{text}</div>
{/if}

{#if app.error}
  <div class="toast err" role="alert">{app.error}</div>
{/if}

<style>
  .toast {
    position: fixed;
    bottom: 16px;
    left: 50%;
    transform: translateX(-50%);
    background: var(--panel);
    color: var(--fg);
    border: var(--border-strong);
    border-left: 3px solid var(--ember);
    border-radius: var(--radius);
    padding: 8px 14px;
    font-size: 12px;
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.4);
    z-index: 100;
    max-width: 70vw;
  }
  .toast.err {
    border-left-color: var(--err);
    color: var(--err);
    bottom: 56px;
  }
  @media (prefers-reduced-motion: no-preference) {
    .toast {
      animation: rise 0.16s ease-out;
    }
  }
  @keyframes rise {
    from {
      opacity: 0;
      transform: translate(-50%, 6px);
    }
    to {
      opacity: 1;
      transform: translate(-50%, 0);
    }
  }
</style>
