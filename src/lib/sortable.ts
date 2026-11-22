/**
 * Drag-to-reorder for a list of direct children, using pointer events
 * (native HTML drag and drop is taken over by Tauri for file drops).
 * Children need `data-id`. A short press-and-move starts the drag; a plain click still clicks.
 */
export type SortableOptions = {
  /** Selector for the reorderable children (direct children of the node). */
  item: string;
  /** Optional selector inside an item that must be grabbed to drag it. */
  handle?: string;
  onsort: (ids: string[]) => void;
};

const THRESHOLD = 5;

export function sortable(node: HTMLElement, options: SortableOptions) {
  let opts = options;
  let cleanup: (() => void) | null = null;

  function items(): HTMLElement[] {
    return Array.from(node.children).filter((c): c is HTMLElement => c instanceof HTMLElement && c.matches(opts.item));
  }

  function onDown(e: PointerEvent) {
    if (e.button !== 0) return;
    const target = e.target as Element;
    const list = items();
    const el = list.find((i) => i.contains(target));
    if (!el) return;
    if (opts.handle) {
      const handle = target.closest(opts.handle);
      if (!handle || !el.contains(handle)) return;
    }
    // Ignore presses that belong to a nested sortable list.
    const nested = target.closest("[data-sortable]");
    if (nested && nested !== node && node.contains(nested)) return;

    const from = list.indexOf(el);
    const startY = e.clientY;
    let started = false;
    let to = from;
    let rects: DOMRect[] = [];
    let step = 0;

    const move = (ev: PointerEvent) => {
      const dy = ev.clientY - startY;
      if (!started) {
        if (Math.abs(dy) < THRESHOLD) return;
        started = true;
        rects = list.map((i) => i.getBoundingClientRect());
        const next = rects[from + 1] ?? rects[from - 1];
        const gap = next ? Math.abs(next.top - rects[from].top) - rects[from].height : 0;
        step = rects[from].height + Math.max(0, gap);
        el.style.position = "relative";
        el.style.zIndex = "5";
        el.style.transition = "none";
        el.dataset.dragging = "1";
        document.body.style.cursor = "grabbing";
        list.forEach((i) => i !== el && (i.style.transition = "transform 160ms cubic-bezier(0.2, 0.8, 0.2, 1)"));
      }
      // Keep the dragged item within the list's bounds.
      const min = rects[0].top - rects[from].top;
      const max = rects[rects.length - 1].bottom - rects[from].bottom;
      const y = Math.max(min, Math.min(max, dy));
      el.style.transform = `translateY(${y}px)`;
      const center = rects[from].top + rects[from].height / 2 + y;
      to = 0;
      rects.forEach((r, i) => {
        if (i !== from && center > r.top + r.height / 2) to++;
      });
      list.forEach((i, idx) => {
        if (i === el) return;
        let shift = 0;
        if (from < to && idx > from && idx <= to) shift = -step;
        if (from > to && idx >= to && idx < from) shift = step;
        i.style.transform = shift ? `translateY(${shift}px)` : "";
      });
    };

    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", up);
      cleanup = null;
      if (!started) return;
      list.forEach((i) => {
        i.style.transform = "";
        i.style.transition = "";
        i.style.zIndex = "";
        i.style.position = "";
        delete i.dataset.dragging;
      });
      document.body.style.cursor = "";
      // Swallow the click that follows a drag (and only that one).
      const swallow = (c: MouseEvent) => {
        c.stopPropagation();
        c.preventDefault();
      };
      window.addEventListener("click", swallow, { capture: true, once: true });
      setTimeout(() => window.removeEventListener("click", swallow, { capture: true }), 0);
      if (to !== from) {
        const ids = list.map((i) => i.dataset.id!);
        const [moved] = ids.splice(from, 1);
        ids.splice(to, 0, moved);
        opts.onsort(ids);
      }
    };

    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", up);
    cleanup = up;
  }

  node.dataset.sortable = "1";
  node.addEventListener("pointerdown", onDown);
  return {
    update(next: SortableOptions) {
      opts = next;
    },
    destroy() {
      node.removeEventListener("pointerdown", onDown);
      cleanup?.();
    },
  };
}
