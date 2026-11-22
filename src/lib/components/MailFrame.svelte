<script lang="ts">
  import { api } from "../api";
  import { frameDoc } from "../html";

  let { html, allowRemote }: { html: string; allowRemote: boolean } = $props();
  let frame: HTMLIFrameElement;
  let h = $state(40);
  let ro: ResizeObserver | undefined;

  const doc = $derived(frameDoc(html, allowRemote));

  function onLoad() {
    const d = frame.contentDocument;
    if (!d) return;
    // Measure the body, not the document: the document is at least as tall as the frame.
    const fit = () => (h = Math.max(24, Math.ceil(d.body.getBoundingClientRect().height)));
    fit();
    ro?.disconnect();
    ro = new ResizeObserver(fit);
    ro.observe(d.body);
    d.querySelectorAll("img").forEach((img) => img.addEventListener("load", fit));
    d.addEventListener("click", (e) => {
      const a = (e.target as Element).closest?.("a");
      if (!a) return;
      e.preventDefault();
      const href = a.getAttribute("href") || "";
      if (/^(https?:|mailto:)/i.test(href)) api.openUrl(href);
    });
  }

  $effect(() => () => ro?.disconnect());
</script>

<iframe
  bind:this={frame}
  title="Message"
  sandbox="allow-same-origin"
  srcdoc={doc}
  onload={onLoad}
  style:height="{h}px"></iframe>

<style>
  iframe {
    display: block;
    width: 100%;
    border: 0;
    background: #fff;
  }
</style>
