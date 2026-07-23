<script lang="ts">
  interface Props {
    close: () => void;
  }
  let { close }: Props = $props();

  const shortcuts: [string, string][] = [
    ["Space", "Play / Stop"],
    ["⌘/Ctrl K", "Command palette"],
    ["⌘/Ctrl Z", "Undo"],
    ["⌘/Ctrl ⇧ Z", "Redo"],
    ["⌘/Ctrl S", "Save set"],
    ["← → ↑ ↓", "Move step selection"],
    ["Enter", "Toggle step at selection"],
    ["Delete / ⌫", "Clear selected step"],
    ["?", "This help"],
  ];
</script>

<div class="scrim" role="presentation" onclick={close}></div>
<div class="help" role="dialog" aria-label="Help">
  <div class="head">
    <h2>midip — help</h2>
    <button class="x" onclick={close} aria-label="Close">✕</button>
  </div>

  <div class="cols">
    <div>
      <h3>Keyboard</h3>
      <table>
        <tbody>
          {#each shortcuts as [k, d] (k)}
            <tr><td class="key mono">{k}</td><td>{d}</td></tr>
          {/each}
        </tbody>
      </table>
    </div>
    <div>
      <h3>Tabs</h3>
      <ul>
        <li><b>Perform</b> — lanes, grid, inspector</li>
        <li><b>Pattern</b> — focused editing</li>
        <li><b>Library</b> — browse / audition / crates</li>
        <li><b>Song</b> — scenes & chains</li>
        <li><b>Setup</b> — routing, clock-in, sets</li>
      </ul>
      <h3>Tips</h3>
      <ul>
        <li>Drag on the drum grid to paint / erase.</li>
        <li>Click a pitch row to place a note; drag it to re-pitch.</li>
        <li>⚡ Generate previews live — Commit or Cancel.</li>
        <li>Mute a lane to audition into it while playing.</li>
      </ul>
    </div>
  </div>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.45);
    z-index: 80;
  }
  .help {
    position: fixed;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    width: 560px;
    max-width: 92vw;
    max-height: 80vh;
    overflow-y: auto;
    background: var(--panel);
    border: var(--border-strong);
    border-radius: 6px;
    box-shadow: 0 16px 48px rgba(0, 0, 0, 0.55);
    z-index: 81;
    padding: 16px 18px;
  }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 12px;
  }
  h2 {
    margin: 0;
    color: var(--ember);
    font-size: 14px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }
  .x {
    background: transparent;
    border: none;
    color: var(--fg-dim);
  }
  .cols {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 22px;
  }
  h3 {
    font-size: 11px;
    text-transform: uppercase;
    color: var(--fg-dim);
    margin: 0 0 6px;
  }
  table {
    border-collapse: collapse;
    width: 100%;
    margin-bottom: 10px;
  }
  td {
    padding: 3px 4px;
    font-size: 12px;
    vertical-align: top;
  }
  .key {
    color: var(--aqua);
    white-space: nowrap;
    padding-right: 10px;
  }
  ul {
    margin: 0 0 12px;
    padding-left: 16px;
  }
  li {
    font-size: 12px;
    line-height: 1.5;
    color: var(--fg);
  }
  b {
    color: var(--ember);
  }
</style>
