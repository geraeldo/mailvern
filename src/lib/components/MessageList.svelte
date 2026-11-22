<script lang="ts">
  import { untrack } from "svelte";
  import Icon from "./Icon.svelte";
  import { ui, openMessage, loadMore, viewTitle, setQuery, openSettings } from "../state.svelte";
  import { hint } from "../shortcuts.svelte";
  import { messageMenu } from "../menus";
  import { shortDate, displayName } from "../format";

  const ROW = 76;
  const OVERSCAN = 8;

  let scroller: HTMLDivElement;
  let scrollTop = $state(0);
  let height = $state(600);
  let searchEl = $state<HTMLInputElement>();
  let searchOpen = $state(false);
  let draft = $state("");
  let loadingMore = false;

  const start = $derived(Math.max(0, Math.floor(scrollTop / ROW) - OVERSCAN));
  const end = $derived(Math.min(ui.list.length, Math.ceil((scrollTop + height) / ROW) + OVERSCAN));
  const visible = $derived(ui.list.slice(start, end));

  async function onScroll() {
    scrollTop = scroller.scrollTop;
    if (!loadingMore && scrollTop + height > ui.list.length * ROW - ROW * 20) {
      loadingMore = true;
      await loadMore();
      loadingMore = false;
    }
  }

  // Bring the selected row into view when the selection changes (keyboard navigation).
  // Only the selection is tracked: list refreshes from sync must not move your scroll.
  let lastSelected: number | null = null;
  $effect(() => {
    const id = ui.selectedId;
    if (id === lastSelected) return;
    lastSelected = id;
    untrack(() => {
      const i = ui.list.findIndex((m) => m.id === id);
      if (i < 0 || !scroller) return;
      const top = i * ROW;
      if (top < scroller.scrollTop) scroller.scrollTop = top;
      else if (top + ROW > scroller.scrollTop + height) scroller.scrollTop = top + ROW - height;
    });
  });

  // Reset scroll when the view changes.
  $effect(() => {
    ui.view;
    if (scroller) scroller.scrollTop = 0;
  });

  let timer: ReturnType<typeof setTimeout>;
  function onInput() {
    clearTimeout(timer);
    timer = setTimeout(() => setQuery(draft), 140);
  }

  export function focusSearch() {
    searchOpen = true;
    queueMicrotask(() => searchEl?.focus());
  }

  function closeSearch() {
    draft = "";
    searchOpen = false;
    if (ui.query) setQuery("");
  }
</script>

<section class="list">
  <header data-tauri-drag-region>
    {#if searchOpen}
      <div class="search">
        <Icon name="search" size={15} />
        <input
          bind:this={searchEl}
          bind:value={draft}
          oninput={onInput}
          onkeydown={(e) => {
            if (e.key === "Escape") closeSearch();
            if (e.key === "ArrowDown" || e.key === "Enter") {
              e.preventDefault();
              searchEl?.blur();
              if (ui.list.length) openMessage(ui.list[0].id);
            }
          }}
          placeholder="Search all mail"
          spellcheck="false" />
        <button class="icon-btn small" onclick={closeSearch} aria-label="Close search"><Icon name="x" size={14} /></button>
      </div>
    {:else}
      <h1 data-tauri-drag-region>{viewTitle()}</h1>
      <button class="icon-btn" title={hint("Search", "search")} onclick={focusSearch}><Icon name="search" /></button>
    {/if}
  </header>

  <div class="scroll body" bind:this={scroller} bind:clientHeight={height} onscroll={onScroll}>
    {#if ui.listReady && !ui.list.length}
      <div class="empty">
        {#if ui.query}
          No results
        {:else if !ui.accounts.length}
          <button class="btn primary" onclick={() => openSettings("new")}>Add your first account</button>
        {:else}
          <Icon name="check" size={22} />
        {/if}
      </div>
    {/if}
    <div style:height="{ui.list.length * ROW + (ui.loadingOlder ? 56 : 0)}px" style:position="relative">
      {#if ui.loadingOlder}
        <div class="older" style:top="{ui.list.length * ROW}px"><span class="spinner"></span></div>
      {/if}
      {#each visible as m, i (m.id)}
        <button
          class="row"
          class:unread={!m.seen}
          class:selected={m.id === ui.selectedId}
          style:transform="translateY({(start + i) * ROW}px)"
          onclick={() => openMessage(m.id)}
          oncontextmenu={(e) => { e.preventDefault(); messageMenu(m); }}>
          <span class="mark">
            {#if !m.seen}<span class="unread-dot"></span>{/if}
          </span>
          <span class="main">
            <span class="line1">
              <span class="from">{displayName(m.fromName, m.fromAddr) || "(no sender)"}</span>
              {#if m.flagged}<span class="flag"><Icon name="flag" size={12} stroke={2} /></span>{/if}
              {#if m.hasAttachments}<span class="clip"><Icon name="clip" size={12} stroke={2} /></span>{/if}
              <span class="date">{shortDate(m.date)}</span>
            </span>
            <span class="subject">{m.subject || "(no subject)"}</span>
            <span class="snippet">
              <span class="snip">{m.snippet}</span>
            </span>
          </span>
        </button>
      {/each}
    </div>
  </div>
</section>

<style>
  .list {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-width: 0;
    border-right: 1px solid var(--line);
    background: var(--panel);
  }
  header {
    height: 52px;
    flex: none;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 10px 0 20px;
  }
  h1 {
    flex: 1;
    margin: 0;
    font-size: 17px;
    font-weight: 650;
    letter-spacing: -0.01em;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .search {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 8px;
    height: 32px;
    margin-left: -8px;
    padding: 0 4px 0 10px;
    border-radius: 8px;
    background: var(--hover);
    color: var(--muted);
  }
  .search input { flex: 1; min-width: 0; border: 0; outline: none; background: transparent; color: var(--text); }
  .icon-btn.small { width: 24px; height: 24px; }
  .body { flex: 1; position: relative; }
  .row {
    position: absolute;
    left: 0;
    right: 0;
    top: 0;
    height: 76px;
    display: flex;
    gap: 2px;
    padding: 11px 16px 0 6px;
    border: 0;
    background: transparent;
    text-align: left;
    color: var(--text);
  }
  .row::after {
    content: "";
    position: absolute;
    left: 20px;
    right: 0;
    bottom: 0;
    height: 1px;
    background: var(--line);
  }
  .row:hover { background: var(--hover); }
  .row.selected { background: var(--selected); }
  .row.selected::after { opacity: 0; }
  .mark { width: 14px; flex: none; display: flex; justify-content: center; padding-top: 6px; }
  .unread-dot { width: 7px; height: 7px; border-radius: 50%; background: var(--accent); }
  .main { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 1px; }
  .line1 { display: flex; align-items: center; gap: 6px; }
  .from {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 14px;
    color: var(--text-2);
  }
  .unread .from { font-weight: 650; color: var(--text); }
  .date { flex: none; font-size: 12px; color: var(--muted); font-variant-numeric: tabular-nums; }
  .flag { color: #e0882a; display: grid; }
  .clip { color: var(--muted); display: grid; }
  .subject, .snippet { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .subject { font-size: 13px; color: var(--text-2); }
  .unread .subject { color: var(--text); font-weight: 500; }
  .snippet { font-size: 12.5px; color: var(--muted); display: flex; align-items: center; gap: 6px; }
  .snip { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .older { position: absolute; left: 0; right: 0; height: 56px; display: grid; place-items: center; }
  .spinner {
    width: 16px;
    height: 16px;
    border-radius: 50%;
    border: 2px solid var(--line-strong);
    border-top-color: var(--muted);
    animation: spin 0.8s linear infinite;
  }
  .empty {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    color: var(--muted);
    font-size: 13px;
  }
</style>
