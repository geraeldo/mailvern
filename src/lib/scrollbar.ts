/**
 * Custom overlay scrollbar for main layout columns.
 *
 * macOS WKWebView fails to repaint `::-webkit-scrollbar-thumb` when transitioning
 * from `transparent` on `:hover` or class changes, and draws permanent classic
 * scrollbars whenever an external mouse is attached or "Show scroll bars: Always"
 * is enabled. Hiding the native scrollbar on `.col-scroll` and rendering a
 * lightweight DOM overlay thumb gives reliable fade-in/out on hover and active
 * scrolling with zero layout shift.
 */
export function overlayScroll(node: HTMLElement) {
  const host = node.parentElement;
  if (!host) return;

  const track = document.createElement("div");
  track.className = "osb-track";
  track.setAttribute("aria-hidden", "true");

  const thumb = document.createElement("div");
  thumb.className = "osb-thumb";
  track.appendChild(thumb);
  host.appendChild(track);

  let hovered = false;
  let scrolling = false;
  let dragging = false;
  let canScroll = false;
  let hideTimer: ReturnType<typeof setTimeout> | undefined;
  let raf = 0;

  let startY = 0;
  let startScrollTop = 0;
  let dragMaxScroll = 0;
  let dragMaxThumbTop = 0;

  function syncVisibility() {
    const show = canScroll && (hovered || scrolling || dragging);
    track.classList.toggle("visible", show);
    track.classList.toggle("dragging", dragging);
  }

  function update() {
    raf = 0;
    if (!hovered && host!.matches(":hover")) hovered = true;
    const ch = node.clientHeight;
    const sh = node.scrollHeight;
    canScroll = ch > 0 && sh > ch + 1;
    if (!canScroll) {
      syncVisibility();
      return;
    }
    const trackH = track.clientHeight || Math.max(0, ch - 4);
    const thumbH = Math.max(28, Math.min(trackH, Math.round((ch / sh) * trackH)));
    const maxScroll = sh - ch;
    const maxTop = Math.max(0, trackH - thumbH);
    const top = maxScroll > 0 ? Math.min(maxTop, Math.max(0, (node.scrollTop / maxScroll) * maxTop)) : 0;

    thumb.style.height = `${thumbH}px`;
    thumb.style.transform = `translateY(${top}px)`;
    syncVisibility();
  }

  function scheduleUpdate() {
    if (!raf) raf = requestAnimationFrame(update);
  }

  function onScroll() {
    scrolling = true;
    update();
    clearTimeout(hideTimer);
    hideTimer = setTimeout(() => {
      scrolling = false;
      syncVisibility();
    }, 800);
  }

  function onEnter() {
    hovered = true;
    update();
  }

  function onLeave() {
    hovered = false;
    syncVisibility();
  }

  function onThumbDown(e: PointerEvent) {
    if (e.button !== 0 || !canScroll) return;
    e.preventDefault();
    e.stopPropagation();
    dragging = true;
    startY = e.clientY;
    startScrollTop = node.scrollTop;
    dragMaxScroll = Math.max(0, node.scrollHeight - node.clientHeight);
    const trackH = track.clientHeight || Math.max(0, node.clientHeight - 4);
    const thumbH = thumb.offsetHeight || 28;
    dragMaxThumbTop = Math.max(1, trackH - thumbH);
    thumb.setPointerCapture(e.pointerId);
    syncVisibility();
  }

  function onThumbMove(e: PointerEvent) {
    if (!dragging) return;
    const dy = e.clientY - startY;
    node.scrollTop = startScrollTop + (dy / dragMaxThumbTop) * dragMaxScroll;
  }

  function onThumbUp(e: PointerEvent) {
    if (!dragging) return;
    dragging = false;
    if (thumb.hasPointerCapture(e.pointerId)) {
      thumb.releasePointerCapture(e.pointerId);
    }
    syncVisibility();
  }

  function onTrackDown(e: PointerEvent) {
    if (e.button !== 0 || e.target !== track || !canScroll) return;
    e.preventDefault();
    const rect = track.getBoundingClientRect();
    const thumbH = thumb.offsetHeight || 28;
    const maxTop = Math.max(1, rect.height - thumbH);
    const clickTop = e.clientY - rect.top - thumbH / 2;
    const ratio = Math.min(1, Math.max(0, clickTop / maxTop));
    node.scrollTop = ratio * (node.scrollHeight - node.clientHeight);
  }

  node.addEventListener("scroll", onScroll, { passive: true });
  host.addEventListener("pointerenter", onEnter);
  host.addEventListener("pointerleave", onLeave);
  thumb.addEventListener("pointerdown", onThumbDown);
  thumb.addEventListener("pointermove", onThumbMove);
  thumb.addEventListener("pointerup", onThumbUp);
  thumb.addEventListener("pointercancel", onThumbUp);
  track.addEventListener("pointerdown", onTrackDown);

  const ro = new ResizeObserver(scheduleUpdate);
  ro.observe(node);

  const mo = new MutationObserver(scheduleUpdate);
  mo.observe(node, { childList: true, subtree: true, attributes: true, attributeFilter: ["style"] });

  scheduleUpdate();

  return {
    destroy() {
      cancelAnimationFrame(raf);
      clearTimeout(hideTimer);
      ro.disconnect();
      mo.disconnect();
      node.removeEventListener("scroll", onScroll);
      host.removeEventListener("pointerenter", onEnter);
      host.removeEventListener("pointerleave", onLeave);
      thumb.removeEventListener("pointerdown", onThumbDown);
      thumb.removeEventListener("pointermove", onThumbMove);
      thumb.removeEventListener("pointerup", onThumbUp);
      thumb.removeEventListener("pointercancel", onThumbUp);
      track.removeEventListener("pointerdown", onTrackDown);
      track.remove();
    },
  };
}
