<script lang="ts">
  import { api } from "../api";
  import { frameDoc } from "../html";

  let { html, allowRemote }: { html: string; allowRemote: boolean } = $props();
  let frame: HTMLIFrameElement;
  let h = $state(200);
  let ro: ResizeObserver | undefined;

  const doc = $derived(frameDoc(html, allowRemote));

  function onLoad() {
    const d = frame.contentDocument;
    if (!d) return;
    const fit = () => (h = Math.max(60, d.documentElement.scrollHeight));
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
