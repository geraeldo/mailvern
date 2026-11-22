<script lang="ts">
  import Icon from "./Icon.svelte";
  import { sanitize } from "../html";
  import type { Snippet } from "svelte";

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

  let linkOpen = $state(false);
  let linkUrl = $state("");
  let saved: Range | null = null;
  let active = $state<Record<string, boolean>>({});

  $effect(() => {
    // Initial content only; the editor owns its DOM afterwards.
    if (el && !el.dataset.ready) {
      el.innerHTML = html;
      el.dataset.ready = "1";
    }
  });

  function exec(cmd: string, value?: string) {
    el?.focus();
    document.execCommand(cmd, false, value);
    refresh();
    oninput?.();
  }

  function refresh() {
    active = {
      bold: document.queryCommandState("bold"),
      italic: document.queryCommandState("italic"),
      underline: document.queryCommandState("underline"),
      list: document.queryCommandState("insertUnorderedList"),
    };
  }

  function onPaste(e: ClipboardEvent) {
    const data = e.clipboardData;
    if (!data) return;
    const h = data.getData("text/html");
    if (h) {
      e.preventDefault();
      document.execCommand("insertHTML", false, sanitize(h));
      oninput?.();
    }
  }

  function startLink() {
    const sel = window.getSelection();
    saved = sel && sel.rangeCount ? sel.getRangeAt(0).cloneRange() : null;
    linkUrl = "";
    linkOpen = true;
  }

  function applyLink() {
    let url = linkUrl.trim();
    linkOpen = false;
    if (!url) return;
    if (!/^(https?:|mailto:)/i.test(url)) url = url.includes("@") && !url.includes("/") ? `mailto:${url}` : `https://${url}`;
    el?.focus();
    const sel = window.getSelection();
    if (saved && sel) {
      sel.removeAllRanges();
      sel.addRange(saved);
    }
    if (sel && sel.isCollapsed) {
      document.execCommand("insertHTML", false, `<a href="${url.replace(/"/g, "&quot;")}">${url.replace(/</g, "&lt;")}</a>`);
    } else {
      document.execCommand("createLink", false, url);
    }
    oninput?.();
  }

  function onKey(e: KeyboardEvent) {
    if (e.metaKey && e.key.toLowerCase() === "k") {
      e.preventDefault();
      startLink();
    }
  }
</script>

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
    onkeydown={onKey}
    onpaste={onPaste}></div>
  <div class="bar">
    <button type="button" class="icon-btn" class:on={active.bold} title="Bold  ⌘B" onmousedown={(e) => e.preventDefault()} onclick={() => exec("bold")}><Icon name="bold" size={15} /></button>
    <button type="button" class="icon-btn" class:on={active.italic} title="Italic  ⌘I" onmousedown={(e) => e.preventDefault()} onclick={() => exec("italic")}><Icon name="italic" size={15} /></button>
    <button type="button" class="icon-btn" class:on={active.underline} title="Underline  ⌘U" onmousedown={(e) => e.preventDefault()} onclick={() => exec("underline")}><Icon name="underline" size={15} /></button>
    <button type="button" class="icon-btn" class:on={active.list} title="List" onmousedown={(e) => e.preventDefault()} onclick={() => exec("insertUnorderedList")}><Icon name="list" size={15} /></button>
    <button type="button" class="icon-btn" title="Link  ⌘K" onmousedown={(e) => e.preventDefault()} onclick={startLink}><Icon name="link" size={15} /></button>
    {#if linkOpen}
      <!-- svelte-ignore a11y_autofocus -->
      <input
        class="link-input"
        placeholder="Paste a link"
        bind:value={linkUrl}
        autofocus
        onkeydown={(e) => {
          if (e.key === "Enter") { e.preventDefault(); applyLink(); }
          if (e.key === "Escape") { e.stopPropagation(); linkOpen = false; el?.focus(); }
        }}
        onblur={() => (linkOpen = false)} />
    {/if}
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
  .area :global(.mv-quote-head) { color: var(--muted); font-size: 13px; margin-bottom: 6px; }
  .area :global(a) { color: var(--accent); }
  .area :global(img) { max-width: 100%; height: auto; }
  .area :global([data-signature]) { color: var(--text-2); }
  .bar { display: flex; align-items: center; gap: 2px; flex: none; padding-top: 10px; border-top: 1px solid var(--line); }
  .extra { margin-left: auto; display: flex; align-items: center; gap: 6px; }
  .link-input {
    margin-left: 6px;
    height: 28px;
    width: 220px;
    padding: 0 9px;
    border: 1px solid var(--line-strong);
    border-radius: 7px;
    background: var(--field);
    outline: none;
    font-size: 13px;
  }
  .link-input:focus { border-color: var(--accent); }
</style>
