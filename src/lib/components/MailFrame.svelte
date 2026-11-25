<script lang="ts">
  import { untrack } from "svelte";
  import { cachedFrameDoc, getCachedFrameHeight, setCachedFrameHeight } from "../mail-cache.svelte";

  let { html, allowRemote, plain = false, cacheKey }: { html: string; allowRemote: boolean; plain?: boolean; cacheKey?: number } = $props();
  let frame: HTMLIFrameElement;
  let h = $state(untrack(() => getCachedFrameHeight(cacheKey) ?? 40));
  let ro: ResizeObserver | undefined;
  let watched: HTMLElement | null = null;

  const doc = $derived(cachedFrameDoc(cacheKey, html, allowRemote, plain));

  // Starts sizing the frame to the current document's body once it has been parsed. Returns false
  // while the frame still shows the previous document (or the initial blank one).
  function track(): boolean {
    const d = frame?.contentDocument;
    const body = d && d.URL === "about:srcdoc" && d.readyState !== "loading" ? d.body : null;
    if (!body || body === watched) return false;
    watched = body;
    ro?.disconnect();
    // Measure the body, not the document: the document is at least as tall as the frame.
    // A body detached by the next document reads as 0 tall, so it no longer counts.
    ro = new ResizeObserver(() => {
      if (!body.isConnected || frame?.contentDocument !== d) return;
      const px = body.getBoundingClientRect().height;
      // A body being torn down or hidden via display:none reads as 0 tall; keep the old height until then.
      if (px > 0) {
        const next = Math.max(24, Math.ceil(px));
        h = next;
        setCachedFrameHeight(cacheKey, next);
      }
    });
    ro.observe(body);
    // No listeners go on `d`: WebKit runs none in a frame sandboxed without scripts. Links reach
    // the browser through the app's navigation hook, and the native menu is trimmed natively.
    return true;
  }

  // Each new document (on opening a message, or after allowing images) is measured as soon as it is
  // parsed, so it never sits cut off while images load. The load event can't be relied on: it waits
  // for every remote image, and one that stalls holds it forever. A timer, not requestAnimationFrame,
  // so it still runs while the window is hidden.
  $effect(() => {
    void doc;
    let timer = 0;
    let tries = 0;
    const tick = () => {
      if (!track() && ++tries < 500) timer = window.setTimeout(tick, 10);
    };
    tick();
    return () => clearTimeout(timer);
  });

  $effect(() => () => ro?.disconnect());
</script>

<iframe
  bind:this={frame}
  title="Message"
  class:plain
  sandbox="allow-same-origin"
  srcdoc={doc}
  onload={track}
  style:height="{h}px"></iframe>

<style>
  iframe {
    display: block;
    width: 100%;
    border: 0;
    background: #fff;
  }
  iframe.plain {
    background: transparent;
  }
</style>
