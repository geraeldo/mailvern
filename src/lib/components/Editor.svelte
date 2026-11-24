<script lang="ts">
  import { open as openDialog } from "@tauri-apps/plugin-dialog";
  import type { Snippet } from "svelte";
  import Icon from "./Icon.svelte";
  import { escapeHtml, sanitize } from "../html";
  import { api } from "../api";
  import { toast } from "../state.svelte";

  let {
    html = "",
    el = $bindable(),
    placeholder = "",
    minHeight = 200,
    oninput,
    children,
  }: {
    html?: string;
    el?: HTMLDivElement;
    placeholder?: string;
    minHeight?: number;
    oninput?: () => void;
    children?: Snippet;
  } = $props();

  type Pop = "style" | "color" | "align" | "link" | null;
  let pop = $state<Pop>(null);
  let linkUrl = $state("");
  let saved: Range | null = null;
  let active = $state<Record<string, boolean>>({});
  let block = $state("div");

  const TEXT_COLORS = ["#1d1c1a", "#6b6862", "#d6402e", "#e0882a", "#c9a400", "#3f8f4a", "#2f9c95", "#2459e0", "#7a5af8", "#c2417a"];
  const HIGHLIGHTS = ["#fff3a3", "#d7f5d0", "#dbe7ff", "#ffdcec", "#ffe3c2"];
  const STYLES = [
    { tag: "div", label: "Text" },
    { tag: "h2", label: "Heading" },
    { tag: "h3", label: "Subheading" },
    { tag: "blockquote", label: "Quote" },
  ];

  $effect(() => {
    // Initial content only; the editor owns its DOM afterwards.
    if (el && !el.dataset.ready) {
      el.innerHTML = html;
      el.dataset.ready = "1";
    }
  });

  function keepSelection() {
    const sel = window.getSelection();
    if (sel && sel.rangeCount && el?.contains(sel.anchorNode)) saved = sel.getRangeAt(0).cloneRange();
  }

  function restoreSelection() {
    el?.focus();
    const sel = window.getSelection();
    if (saved && sel) {
      sel.removeAllRanges();
      sel.addRange(saved);
    }
  }

  function exec(cmd: string, value?: string) {
    restoreSelection();
    document.execCommand("styleWithCSS", false, "true");
    document.execCommand(cmd, false, value);
    pop = null;
    refresh();
    oninput?.();
  }

  function refresh() {
    keepSelection();
    active = {
      bold: document.queryCommandState("bold"),
      italic: document.queryCommandState("italic"),
      underline: document.queryCommandState("underline"),
      strike: document.queryCommandState("strikeThrough"),
      ul: document.queryCommandState("insertUnorderedList"),
      ol: document.queryCommandState("insertOrderedList"),
    };
    const b = String(document.queryCommandValue("formatBlock") || "div").toLowerCase();
    block = STYLES.some((s) => s.tag === b) ? b : "div";
  }

  function toggle(p: Pop) {
    keepSelection();
    pop = pop === p ? null : p;
    if (p === "link") linkUrl = "";
  }

  function applyLink() {
    let url = linkUrl.trim();
    pop = null;
    if (!url) return;
    if (!/^(https?:|mailto:|tel:)/i.test(url)) url = url.includes("@") && !url.includes("/") ? `mailto:${url}` : `https://${url}`;
    restoreSelection();
    const sel = window.getSelection();
    const safe = escapeHtml(url);
    if (sel && sel.isCollapsed) {
      document.execCommand("insertHTML", false, `<a href="${safe}">${safe}</a>`);
    } else {
      document.execCommand("createLink", false, url);
    }
    oninput?.();
  }

  /** Downscales large images so messages stay a sensible size. */
  async function shrink(dataUrl: string): Promise<{ src: string; width: number }> {
    const img = new Image();
    img.src = dataUrl;
    await img.decode();
    const max = 1600;
    const big = img.naturalWidth > max || dataUrl.length > 1_500_000;
    let src = dataUrl;
    if (big) {
      const scale = Math.min(1, max / img.naturalWidth);
      const c = document.createElement("canvas");
      c.width = Math.round(img.naturalWidth * scale);
      c.height = Math.round(img.naturalHeight * scale);
      c.getContext("2d")!.drawImage(img, 0, 0, c.width, c.height);
      src = dataUrl.startsWith("data:image/png") && c.width * c.height < 1_000_000 ? c.toDataURL("image/png") : c.toDataURL("image/jpeg", 0.85);
    }
    return { src, width: Math.min(img.naturalWidth, 600) };
  }

  async function insertImage(dataUrl: string) {
    try {
      const { src, width } = await shrink(dataUrl);
      restoreSelection();
      document.execCommand("insertHTML", false, `<img src="${src}" width="${width}" style="max-width:100%;height:auto" alt="">`);
      oninput?.();
    } catch {
      toast("Couldn't read that image");
    }
  }

  async function pickImage() {
    keepSelection();
    const path = await openDialog({ multiple: false, filters: [{ name: "Images", extensions: ["png", "jpg", "jpeg", "gif", "webp"] }] });
    if (!path || Array.isArray(path)) return;
    try {
      await insertImage(await api.readImage(path));
    } catch (e) {
      toast(String(e));
    }
  }

  function onPaste(e: ClipboardEvent) {
    const data = e.clipboardData;
    if (!data) return;
    const file = Array.from(data.files).find((f) => f.type.startsWith("image/"));
    if (file) {
      e.preventDefault();
      keepSelection();
      const r = new FileReader();
      r.onload = () => insertImage(String(r.result));
      r.readAsDataURL(file);
      return;
    }
    const h = data.getData("text/html");
    if (h) {
      e.preventDefault();
      document.execCommand("insertHTML", false, sanitize(h));
      oninput?.();
    }
  }

  function onKey(e: KeyboardEvent) {
    if (e.metaKey && e.key.toLowerCase() === "k") {
      e.preventDefault();
      toggle("link");
    } else if (e.metaKey && e.shiftKey && e.key.toLowerCase() === "x") {
      e.preventDefault();
      exec("strikeThrough");
    } else if (e.metaKey && e.shiftKey && (e.key === "7" || e.key === "&")) {
      e.preventDefault();
      exec("insertOrderedList");
    } else if (e.metaKey && e.shiftKey && (e.key === "8" || e.key === "*")) {
      e.preventDefault();
      exec("insertUnorderedList");
    }
  }

  const keep = (e: MouseEvent) => e.preventDefault();
</script>

<svelte:window onmousedown={(e) => { if (pop && !(e.target as Element).closest?.(".pop, .has-pop")) pop = null; }} />

<div class="editor">
  <div
    bind:this={el}
    class="area scroll"
    contenteditable="true"
    role="textbox"
    aria-multiline="true"
    tabindex="0"
    data-placeholder={placeholder}
    style:min-height="{minHeight}px"
    oninput={() => oninput?.()}
    onkeyup={refresh}
    onmouseup={refresh}
    onblur={keepSelection}
    onkeydown={onKey}
    onpaste={onPaste}></div>

  <div class="bar">
    <div class="tools">
      <span class="has-pop">
        <button type="button" class="pick" title="Text style" onmousedown={keep} onclick={() => toggle("style")}>
          {STYLES.find((s) => s.tag === block)?.label ?? "Text"}
          <span class="caret"><Icon name="chevron" size={10} stroke={2.4} /></span>
        </button>
        {#if pop === "style"}
          <div class="pop menu">
            {#each STYLES as s (s.tag)}
              <button type="button" class="opt {s.tag}" class:on={block === s.tag} onmousedown={keep} onclick={() => exec("formatBlock", s.tag)}>{s.label}</button>
            {/each}
          </div>
        {/if}
      </span>
      <span class="sep"></span>
      <button type="button" class="icon-btn" class:on={active.bold} title="Bold  ⌘B" onmousedown={keep} onclick={() => exec("bold")}><Icon name="bold" size={15} /></button>
      <button type="button" class="icon-btn" class:on={active.italic} title="Italic  ⌘I" onmousedown={keep} onclick={() => exec("italic")}><Icon name="italic" size={15} /></button>
      <button type="button" class="icon-btn" class:on={active.underline} title="Underline  ⌘U" onmousedown={keep} onclick={() => exec("underline")}><Icon name="underline" size={15} /></button>
      <button type="button" class="icon-btn" class:on={active.strike} title="Strikethrough  ⌘⇧X" onmousedown={keep} onclick={() => exec("strikeThrough")}><Icon name="strike" size={15} /></button>
      <span class="has-pop">
        <button type="button" class="icon-btn" title="Color" onmousedown={keep} onclick={() => toggle("color")}>
          <span class="color-a">A</span>
        </button>
        {#if pop === "color"}
          <div class="pop colors">
            <div class="clabel">Text</div>
            <div class="swatches">
              {#each TEXT_COLORS as c (c)}
                <button type="button" class="sw" style:background={c} aria-label="Text color" onmousedown={keep} onclick={() => exec("foreColor", c)}></button>
              {/each}
            </div>
            <div class="clabel">Highlight</div>
            <div class="swatches">
              <button type="button" class="sw none" aria-label="No highlight" onmousedown={keep} onclick={() => exec("hiliteColor", "transparent")}></button>
              {#each HIGHLIGHTS as c (c)}
                <button type="button" class="sw" style:background={c} aria-label="Highlight" onmousedown={keep} onclick={() => exec("hiliteColor", c)}></button>
              {/each}
            </div>
          </div>
        {/if}
      </span>
      <span class="sep"></span>
      <button type="button" class="icon-btn" class:on={active.ul} title="Bulleted list  ⌘⇧8" onmousedown={keep} onclick={() => exec("insertUnorderedList")}><Icon name="list" size={15} /></button>
      <button type="button" class="icon-btn" class:on={active.ol} title="Numbered list  ⌘⇧7" onmousedown={keep} onclick={() => exec("insertOrderedList")}><Icon name="olist" size={15} /></button>
      <span class="has-pop">
        <button type="button" class="icon-btn" title="Align" onmousedown={keep} onclick={() => toggle("align")}><Icon name="alignLeft" size={15} /></button>
        {#if pop === "align"}
          <div class="pop row">
            <button type="button" class="icon-btn" title="Left" onmousedown={keep} onclick={() => exec("justifyLeft")}><Icon name="alignLeft" size={15} /></button>
            <button type="button" class="icon-btn" title="Center" onmousedown={keep} onclick={() => exec("justifyCenter")}><Icon name="alignCenter" size={15} /></button>
            <button type="button" class="icon-btn" title="Right" onmousedown={keep} onclick={() => exec("justifyRight")}><Icon name="alignRight" size={15} /></button>
          </div>
        {/if}
      </span>
      <span class="sep"></span>
      <span class="has-pop">
        <button type="button" class="icon-btn" title="Link  ⌘K" onmousedown={keep} onclick={() => toggle("link")}><Icon name="link" size={15} /></button>
        {#if pop === "link"}
          <div class="pop row">
            <!-- svelte-ignore a11y_autofocus -->
            <input
              class="link-input"
              placeholder="Paste a link"
              bind:value={linkUrl}
              autofocus
              onkeydown={(e) => {
                if (e.key === "Enter") { e.preventDefault(); applyLink(); }
                if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); pop = null; restoreSelection(); }
              }} />
            <button type="button" class="btn small" onclick={applyLink}>Add</button>
          </div>
        {/if}
      </span>
      <button type="button" class="icon-btn" title="Insert image" onmousedown={keep} onclick={pickImage}><Icon name="image" size={15} /></button>
      <button type="button" class="icon-btn" title="Clear formatting" onmousedown={keep} onclick={() => { exec("removeFormat"); exec("formatBlock", "div"); }}><Icon name="eraser" size={15} /></button>
    </div>
    {#if children}<div class="extra">{@render children()}</div>{/if}
  </div>
</div>

<style>
  .editor { display: flex; flex-direction: column; min-height: 0; flex: 1; }
  .area {
    flex: 1;
    overflow-y: auto;
    outline: none;
    padding: 4px 0 16px;
    font-size: 14.5px;
    line-height: 1.6;
    overflow-wrap: anywhere;
  }
  .area:empty::before { content: attr(data-placeholder); color: var(--muted); pointer-events: none; }
  .area :global(blockquote) {
    margin: 0 0 0 2px;
    padding-left: 12px;
    border-left: 2px solid var(--line-strong);
    color: var(--text-2);
  }
  .area :global(h2) { font-size: 19px; line-height: 1.3; margin: 10px 0 4px; font-weight: 650; }
  .area :global(h3) { font-size: 16px; line-height: 1.35; margin: 8px 0 2px; font-weight: 600; }
  .area :global(ul), .area :global(ol) { padding-left: 24px; margin: 4px 0; }
  .area :global(.mv-quote-head) { color: var(--muted); font-size: 13px; margin-bottom: 6px; }
  .area :global(a) { color: var(--accent); }
  .area :global(img) { max-width: 100%; height: auto; border-radius: 2px; }
  .area :global(img:hover) { outline: 2px solid var(--accent-soft); }
  .area :global([data-signature]) { color: var(--text-2); }

  .bar {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: none;
    padding-top: 10px;
    border-top: 1px solid var(--line);
  }
  .tools { display: flex; align-items: center; gap: 1px; flex-wrap: wrap; min-width: 0; }
  .tools .icon-btn { width: 28px; height: 28px; }
  .sep { width: 1px; height: 16px; background: var(--line); margin: 0 5px; }
  .extra { margin-left: auto; display: flex; align-items: center; gap: 6px; flex: none; }
  .has-pop { position: relative; display: inline-flex; }
  .pick {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    height: 28px;
    padding: 0 6px 0 8px;
    border: 0;
    border-radius: 7px;
    background: transparent;
    color: var(--text-2);
    font-size: 13px;
    font-weight: 500;
  }
  .pick:hover { background: var(--hover); color: var(--text); }
  .caret { display: grid; transform: rotate(90deg); color: var(--muted); }
  .color-a {
    font: 600 14px/1 var(--font);
    padding-bottom: 2px;
    border-bottom: 3px solid #d6402e;
    color: var(--text-2);
  }
  .pop {
    position: absolute;
    bottom: calc(100% + 8px);
    left: -4px;
    z-index: 20;
    padding: 6px;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 10px;
    box-shadow: var(--shadow);
    animation: rise 0.12s ease-out;
  }
  .pop.row { display: flex; align-items: center; gap: 4px; }
  .menu { display: flex; flex-direction: column; min-width: 150px; }
  .opt {
    text-align: left;
    padding: 6px 10px;
    border: 0;
    border-radius: 7px;
    background: transparent;
    font-size: 13.5px;
  }
  .opt:hover { background: var(--hover); }
  .opt.on { background: var(--selected); }
  .opt.h2 { font-size: 17px; font-weight: 650; }
  .opt.h3 { font-size: 15px; font-weight: 600; }
  .opt.blockquote { color: var(--text-2); border-left: 2px solid var(--line-strong); border-radius: 0 7px 7px 0; }
  .colors { width: 214px; padding: 10px; }
  .clabel { font-size: 11.5px; font-weight: 600; color: var(--muted); margin: 0 0 6px; }
  .clabel + .swatches { margin-bottom: 10px; }
  .swatches { display: flex; flex-wrap: wrap; gap: 6px; }
  .swatches:last-child { margin-bottom: 0; }
  .sw {
    width: 22px;
    height: 22px;
    padding: 0;
    border: 0;
    border-radius: 6px;
    box-shadow: inset 0 0 0 1px rgba(0, 0, 0, 0.1);
  }
  .sw:hover { transform: scale(1.1); }
  .sw.none {
    background: linear-gradient(135deg, transparent 45%, #d6402e 45%, #d6402e 55%, transparent 55%), var(--field);
  }
  .link-input {
    height: 30px;
    width: 240px;
    padding: 0 9px;
    border: 1px solid var(--line-strong);
    border-radius: 7px;
    background: var(--field);
    outline: none;
    font-size: 13px;
  }
  .link-input:focus { border-color: var(--accent); }
  .btn.small { height: 30px; padding: 0 12px; }
</style>
