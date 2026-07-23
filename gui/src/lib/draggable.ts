// Svelte action: make `node` draggable by a handle (default: the whole node).
// On the first drag it switches the element to fixed left/top positioning and
// clears any centering transform, so CSS can center it initially and the user
// can then move it anywhere. Drags starting on interactive controls are ignored.

interface Opts {
  handle?: string; // CSS selector for the drag handle within `node`
}

export function draggable(node: HTMLElement, opts: Opts = {}) {
  let sx = 0;
  let sy = 0;
  let ox = 0;
  let oy = 0;
  let dragging = false;

  function onMove(e: MouseEvent) {
    if (!dragging) return;
    node.style.left = `${ox + (e.clientX - sx)}px`;
    node.style.top = `${oy + (e.clientY - sy)}px`;
  }
  function onUp() {
    dragging = false;
    window.removeEventListener("mousemove", onMove);
    window.removeEventListener("mouseup", onUp);
  }
  function onDown(e: MouseEvent) {
    const target = e.target as Element | null;
    if (opts.handle && !target?.closest(opts.handle)) return;
    // Never start a drag from a control the user is actually using.
    if (target?.closest("button, input, select, textarea")) return;

    const r = node.getBoundingClientRect();
    ox = r.left;
    oy = r.top;
    sx = e.clientX;
    sy = e.clientY;
    node.style.position = "fixed";
    node.style.margin = "0";
    node.style.transform = "none";
    node.style.left = `${ox}px`;
    node.style.top = `${oy}px`;
    dragging = true;
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    e.preventDefault();
  }

  node.addEventListener("mousedown", onDown);
  return {
    destroy() {
      node.removeEventListener("mousedown", onDown);
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    },
  };
}
