<script lang="ts">
  import { untrack } from "svelte";
  import Icon from "./Icon.svelte";
  import Avatar from "./Avatar.svelte";
  import {
    ui, openMessage, loadMore, viewTitle, setQuery, openAccountSettings, openSettings, clickRow, isSelected, roleOf,
    editDraft, accountById, accountLabel, beginSearch, searchScopes, scopeKey, scopeLabel, setSearchScope, setView,
    archiveRows, trashRows, toggleSeenRows, snoozeTargetRows, unsnoozeRows, compose, toggleSidebar,
    aiSourceTurn, aiRunning,
  } from "../state.svelte";
  import { hint } from "../shortcuts.svelte";
  import { messageMenu } from "../menus";
  import { shortDate, displayName } from "../format";
  import { whenLabel } from "../when";
  import { addrOf } from "../avatars.svelte";
  import { api, type Summary } from "../api";
  import { ai, searchReady } from "../ai/config.svelte";
  import { runAiSearch, stopAiSearch } from "../ai/search.svelte";

  const ROW = 78;
  const OVERSCAN = 8;

  let scroller = $state<HTMLDivElement>();
  let scrollTop = $state(0);
  let height = $state(600);
  let searchEl = $state<HTMLInputElement>();
  // Picks up a search already in progress (an AI answer, or words typed before a reload).
  let searchOpen = $state(!!ui.query || !!ui.ai);
  let draft = $state(ui.query);
  let loadingMore = false;

  const start = $derived(Math.max(0, Math.floor(scrollTop / ROW) - OVERSCAN));
  const end = $derived(Math.min(ui.list.length, Math.ceil((scrollTop + height) / ROW) + OVERSCAN));
  const visible = $derived(ui.list.slice(start, end));
  /** Show quiet account tags on rows when viewing multiple accounts together. */
  const showAccountTag = $derived(
    ui.accounts.length > 1 && (ui.view.kind === "unified" || (!!ui.query.trim() && ui.searchScope.kind !== "account" && ui.searchScope.kind !== "folder") || !!ui.ai),
  );
  const autoAskAi = $derived(ai.hasKey && draft.trim().endsWith("?"));

  async function onScroll() {
    if (!scroller) return;
    scrollTop = scroller.scrollTop;
    if (!loadingMore && scrollTop + height > ui.list.length * ROW - ROW * 20) {
      loadingMore = true;
      try {
        await loadMore();
      } finally {
        loadingMore = false;
      }
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

  /** Double-click a draft to keep writing it. */
  async function onDouble(id: number) {
    const row = ui.list.find((m) => m.id === id);
    if (roleOf(row) !== "drafts") return;
    try {
      editDraft(ui.detail?.id === id ? ui.detail : await api.message(id));
    } catch {}
  }

  const outboxTo = (to: string[]) => to.map((t) => t.replace(/\s*<.*>$/, "").replace(/^"|"$/g, "")).join(", ") || "(no recipients)";

  // Reset scroll when the view or the search changes.
  $effect(() => {
    ui.view;
    ui.query;
    scrollTop = 0;
    if (scroller) scroller.scrollTop = 0;
  });

  let timer: ReturnType<typeof setTimeout>;
  let lastQuery = ui.query;

  // Pick up queries set from outside the search box (e.g. "Search for This Address").
  $effect(() => {
    const q = ui.query;
    if (q && q !== lastQuery) {
      lastQuery = q;
      clearTimeout(timer);
      searchOpen = true;
      draft = q;
    } else if (!q) {
      lastQuery = "";
    }
  });

  function onInput() {
    stopAiSearch();
    clearTimeout(timer);
    // Search once typing pauses, not on every key.
    timer = setTimeout(() => {
      lastQuery = draft;
      setQuery(draft);
    }, 250);
  }

  export function focusSearch() {
    // Scheduled mail isn't searchable: search from the inboxes instead.
    if (ui.view.kind === "outbox") setView({ kind: "unified", role: "inbox" });
    if (!searchOpen) beginSearch();
    searchOpen = true;
    queueMicrotask(() => searchEl?.focus());
  }

  function askAi() {
    const q = draft.trim();
    if (!q) return;
    if (!searchReady()) {
      openSettings("ai");
      return;
    }
    clearTimeout(timer);
    // The words stay as the query, so closing the answer goes back to their results.
    lastQuery = draft;
    if (ui.query !== draft) setQuery(draft);
    searchEl?.blur();
    runAiSearch(q);
  }

  /** Sources are numbered to match the answer's citations, once the answer is in. */
  const sourceTurn = $derived(aiSourceTurn(ui.ai));
  const numbered = $derived(!!sourceTurn && !sourceTurn.running);
  const running = $derived(aiRunning(ui.ai));

  function closeSearch() {
    clearTimeout(timer);
    stopAiSearch();
    lastQuery = "";
    draft = "";
    searchOpen = false;
    if (ui.query) setQuery("");
  }

  function rowKey(m: Summary, e: KeyboardEvent) {
    if (e.target !== e.currentTarget) return;
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      openMessage(m.id);
    }
  }
</script>

<section class="list">
  <header class:collapsed-side={ui.sidebarCollapsed} data-tauri-drag-region>
    {#if ui.sidebarCollapsed && !searchOpen}
      <button type="button" class="icon-btn" title="Show sidebar  ⌥⌘S" aria-label="Show sidebar" onclick={toggleSidebar}>
        <Icon name="sidebar" size={16} />
      </button>
      <button type="button" class="icon-btn" title={hint("New message", "compose")} aria-label="New message" onclick={() => compose()}>
        <Icon name="compose" size={16} />
      </button>
    {/if}
    {#if searchOpen}
      <div class="search">
        <Icon name="search" size={15} />
        <input
          bind:this={searchEl}
          bind:value={draft}
          oninput={onInput}
          onkeydown={(e) => {
            if (e.key === "Escape") closeSearch();
            // ⌘↩, or a question (ending in "?"), asks AI.
            if (e.key === "Enter" && (e.metaKey || autoAskAi)) {
              e.preventDefault();
              askAi();
              return;
            }
            if (e.key === "ArrowDown" || e.key === "Enter") {
              e.preventDefault();
              searchEl?.blur();
              if (ui.list.length) openMessage(ui.list[0].id);
            }
          }}
          placeholder="Search {scopeLabel(ui.searchScope)}"
          spellcheck="false" />
        <button type="button" class="icon-btn small" onclick={closeSearch} aria-label="Close search"><Icon name="x" size={14} /></button>
      </div>
    {:else}
      <h1 data-tauri-drag-region>{viewTitle()}</h1>
      <button type="button" class="icon-btn" title={hint("Search", "search")} aria-label="Search" onclick={focusSearch}><Icon name="search" /></button>
    {/if}
  </header>

  {#if searchOpen && !ui.ai}
    <div class="scopes" role="radiogroup" aria-label="Search in">
      {#each searchScopes() as s (scopeKey(s))}
        {@const on = scopeKey(s) === scopeKey(ui.searchScope)}
        <button type="button" class="scope" class:on role="radio" aria-checked={on} onclick={() => setSearchScope(s)}>
          {#if s.kind === "account"}<span class="scope-bar" style:background={accountById(s.accountId)?.color || "var(--muted)"}></span>{/if}
          {scopeLabel(s)}
        </button>
      {/each}
    </div>
  {/if}

  {#if searchOpen && draft.trim() && !ui.ai}
    <button type="button" class="ask" onclick={askAi}>
      <Icon name="sparkle" size={14} />
      <span class="ask-text">{ai.hasKey ? "Ask AI" : "Set up AI search"}<span class="ask-q">{ai.hasKey ? ` “${draft.trim()}”` : ""}</span></span>
      <span class="kbd">{autoAskAi ? "↩" : "⌘↩"}</span>
    </button>
  {/if}
  {#if ui.ai}
    <div class="sources-head">
      {#if running && !ui.list.length}
        <span class="spinner small"></span> Finding sources…
      {:else}
        <Icon name="sparkle" size={13} />
        {ui.list.length} source{ui.list.length === 1 ? "" : "s"}{running ? " so far" : ""}
      {/if}
    </div>
  {/if}

  {#if ui.view.kind === "outbox"}
    <div class="scroll body">
      {#each ui.outbox as o (o.id)}
        <button type="button" class="row out" class:selected={o.id === ui.outboxSelected} onclick={() => (ui.outboxSelected = o.id)}>
          <span class="mark">{#if o.error}<span class="err-dot" title={o.error}></span>{/if}</span>
          <span class="pic"><Avatar name={outboxTo(o.message.to.slice(0, 1))} email={addrOf(o.message.to[0] ?? o.message.cc[0] ?? "")} size={34} /></span>
          <span class="main">
            <span class="line1">
              <span class="from">To: {outboxTo(o.message.to.concat(o.message.cc))}</span>
              <span class="date" class:late={!!o.error}><Icon name="clock" size={11} stroke={2} /> {whenLabel(new Date(o.sendAt * 1000))}</span>
            </span>
            <span class="subject">{o.message.subject || "(no subject)"}</span>
            <span class="snippet"><span class="snip">{o.error ? `Not sent: ${o.error}` : (accountById(o.accountId)?.email ?? "")}</span></span>
          </span>
        </button>
      {:else}
        <div class="empty">
          <Icon name="check" size={22} />
          <p class="empty-title">No scheduled mail</p>
        </div>
      {/each}
    </div>
  {:else}
    <div class="scroll body" bind:this={scroller} bind:clientHeight={height} onscroll={onScroll}>
      {#if ui.listReady && !ui.list.length && !ui.searchingServer && !running}
        <div class="empty">
          {#if ui.ai}
            <p class="empty-title">No sources found</p>
            <p class="empty-sub">Try phrasing the question differently.</p>
          {:else if ui.query}
            <p class="empty-title">No results for “{ui.query.trim()}”</p>
            <p class="empty-sub">Try another scope above, or press ⌘↩ to ask AI.</p>
          {:else if !ui.accounts.length}
            <p class="empty-title">Welcome to Mailvern</p>
            <button type="button" class="btn primary" onclick={() => openAccountSettings("new")}>Add your first account</button>
          {:else}
            <span class="empty-icon"><Icon name="check" size={20} /></span>
            <p class="empty-title">{viewTitle() === "Inbox" ? "All caught up" : `Nothing in ${viewTitle()}`}</p>
            {#if viewTitle() === "Inbox"}
              <p class="empty-sub">Enjoy the quiet.</p>
            {/if}
          {/if}
        </div>
      {/if}
      <div style:height="{ui.list.length * ROW + (ui.loadingOlder || ui.searchingServer ? 56 : 0)}px" style:position="relative">
        {#if ui.loadingOlder}
          <div class="older" style:top="{ui.list.length * ROW}px"><span class="spinner"></span></div>
        {:else if ui.searchingServer}
          <div class="older" style:top="{ui.list.length * ROW}px"><span class="spinner"></span><span class="searching">Searching servers…</span></div>
        {/if}
        {#each visible as m, i (m.id)}
          {@const rowRole = roleOf(m)}
          {@const acc = showAccountTag ? accountById(m.accountId) : undefined}
          <div
            role="button"
            tabindex="0"
            class="row"
            class:unread={!m.seen}
            class:selected={isSelected(m.id)}
            class:multi={ui.selectedIds.length > 1}
            style:transform="translateY({(start + i) * ROW}px)"
            onclick={(e) => clickRow(m, e)}
            onkeydown={(e) => rowKey(m, e)}
            ondblclick={() => onDouble(m.id)}
            oncontextmenu={(e) => { e.preventDefault(); messageMenu(m); }}>
            <span class="mark">
              {#if ui.ai && numbered}<span class="n">{start + i + 1}</span>{:else if !m.seen}<span class="unread-dot" aria-label="Unread"></span>{/if}
            </span>
            <span class="pic"><Avatar name={m.fromName} email={m.fromAddr} size={34} lookup={rowRole !== "junk"} /></span>
            <span class="main">
              <span class="line1">
                <span class="from">
                  {#if rowRole === "drafts"}<span class="draft">Draft</span>{/if}
                  {displayName(m.fromName, m.fromAddr) || "(no sender)"}
                </span>
                {#if m.count > 1}<span class="count">{m.count}</span>{/if}
                <span class="meta-right">
                  {#if m.flagged}<span class="flag"><Icon name="flag" size={12} stroke={2} /></span>{/if}
                  {#if m.hasAttachments}<span class="clip"><Icon name="clip" size={12} stroke={2} /></span>{/if}
                  {#if m.snoozedUntil}
                    <span class="date snoozed" title="Comes back {whenLabel(new Date(m.snoozedUntil * 1000))}"><Icon name="clock" size={11} stroke={2} /> {whenLabel(new Date(m.snoozedUntil * 1000))}</span>
                  {:else}
                    <span class="date">{shortDate(m.date)}</span>
                  {/if}
                </span>
                <span class="quick" role="group" aria-label="Quick actions">
                  {#if rowRole !== "archive"}
                    <button type="button" class="qbtn" title={hint("Archive", "archive")} aria-label="Archive" onclick={(e) => { e.stopPropagation(); archiveRows([m]); }}>
                      <Icon name="archive" size={13} />
                    </button>
                  {/if}
                  {#if rowRole !== "trash"}
                    <button type="button" class="qbtn" title={hint("Delete", "delete")} aria-label="Delete" onclick={(e) => { e.stopPropagation(); trashRows([m]); }}>
                      <Icon name="trash" size={13} />
                    </button>
                  {/if}
                  <button
                    type="button"
                    class="qbtn"
                    title={m.seen ? "Mark as unread" : "Mark as read"}
                    aria-label={m.seen ? "Mark as unread" : "Mark as read"}
                    onclick={(e) => { e.stopPropagation(); toggleSeenRows([m]); }}>
                    <Icon name={m.seen ? "unread" : "mailOpen"} size={13} />
                  </button>
                  {#if rowRole !== "snoozed"}
                    <button type="button" class="qbtn" title={hint("Snooze", "snooze")} aria-label="Snooze" onclick={(e) => { e.stopPropagation(); snoozeTargetRows([m]); }}>
                      <Icon name="clock" size={13} />
                    </button>
                  {:else}
                    <button type="button" class="qbtn" title="Unsnooze" aria-label="Unsnooze" onclick={(e) => { e.stopPropagation(); unsnoozeRows([m]); }}>
                      <Icon name="clock" size={13} />
                    </button>
                  {/if}
                </span>
              </span>
              <span class="subject">{m.subject || "(no subject)"}</span>
              <span class="snippet">
                <span class="snip">{m.snippet}</span>
                {#if acc}
                  <span class="acc-tag" title={acc.email}>
                    <span class="acc-bar" style:background={acc.color || "var(--muted)"}></span>
                    <span class="acc-name">{accountLabel(acc)}</span>
                  </span>
                {/if}
              </span>
            </span>
          </div>
        {/each}
      </div>
    </div>
  {/if}
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
    gap: 6px;
    padding: 0 10px 0 20px;
  }
  /* Leave clear space for macOS traffic lights when the sidebar is collapsed. */
  header.collapsed-side { padding-left: 84px; }
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
  header.collapsed-side .search { margin-left: 0; }
  .search input { flex: 1; min-width: 0; border: 0; outline: none; background: transparent; color: var(--text); }
  .icon-btn.small { width: 24px; height: 24px; }
  .scopes {
    flex: none;
    display: flex;
    gap: 6px;
    padding: 0 12px 10px;
    overflow-x: auto;
    scrollbar-width: none;
  }
  .scopes::-webkit-scrollbar { display: none; }
  .scope {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 24px;
    padding: 0 10px;
    border: 1px solid var(--line);
    border-radius: 12px;
    background: transparent;
    color: var(--muted);
    font-size: 12px;
    white-space: nowrap;
    cursor: default;
  }
  .scope:hover { background: var(--hover); color: var(--text); }
  .scope.on { background: var(--text); border-color: var(--text); color: var(--panel); }
  .scope-bar { width: 3px; height: 10px; border-radius: 2px; flex: none; }
  .ask {
    flex: none;
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0 12px 10px;
    padding: 0 10px;
    height: 32px;
    border: 1px solid var(--line);
    border-radius: 8px;
    background: transparent;
    color: var(--accent);
    font-size: 13px;
    text-align: left;
  }
  .ask:hover { background: var(--accent-soft); border-color: transparent; }
  .ask-text { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-weight: 500; }
  .ask-q { color: var(--text-2); font-weight: 400; }
  .kbd { flex: none; color: var(--muted); font-size: 11.5px; }
  .body { flex: 1; position: relative; }
  .row {
    position: absolute;
    left: 0;
    right: 0;
    top: 0;
    height: 78px;
    display: flex;
    gap: 2px;
    padding: 11px 14px 0 4px;
    border: 0;
    background: transparent;
    text-align: left;
    color: var(--text);
    outline-offset: -2px;
  }
  .row::after {
    content: "";
    position: absolute;
    left: 64px;
    right: 0;
    bottom: 0;
    height: 1px;
    background: var(--line);
  }
  .row:hover { background: var(--hover); }
  .row.selected { background: var(--selected); }
  .row.selected.multi { box-shadow: inset 3px 0 0 var(--accent); }
  .out { position: relative; width: 100%; }
  .err-dot { width: 7px; height: 7px; border-radius: 50%; background: var(--danger); }
  .count {
    flex: none;
    min-width: 18px;
    height: 16px;
    padding: 0 5px;
    border-radius: 8px;
    background: var(--hover);
    color: var(--muted);
    font-size: 11px;
    font-weight: 600;
    line-height: 16px;
    text-align: center;
    font-variant-numeric: tabular-nums;
  }
  .draft { color: var(--danger); font-weight: 600; margin-right: 4px; }
  .date.snoozed, .date.late { display: inline-flex; align-items: center; gap: 3px; color: var(--warn); }
  .date.late { color: var(--danger); }
  .searching { margin-left: 8px; font-size: 12px; color: var(--muted); }
  .row.selected::after { opacity: 0; }
  /* Fixed mark width keeps avatar and text columns perfectly aligned across normal & AI citation views. */
  .mark { width: 20px; flex: none; display: flex; justify-content: center; padding-top: 6px; }
  .unread-dot { width: 7px; height: 7px; border-radius: 50%; background: var(--accent); }
  .n {
    min-width: 17px;
    height: 17px;
    margin-top: -3px;
    padding: 0 4px;
    border-radius: 5px;
    background: var(--accent-soft);
    color: var(--accent);
    font-size: 11px;
    font-weight: 650;
    line-height: 17px;
    text-align: center;
  }
  .sources-head {
    flex: none;
    display: flex;
    align-items: center;
    gap: 7px;
    height: 30px;
    padding: 0 20px 8px;
    border-bottom: 1px solid var(--line);
    color: var(--muted);
    font-size: 12px;
  }
  .sources-head :global(svg) { color: var(--accent); }
  .spinner.small { width: 11px; height: 11px; border-width: 1.5px; }
  .pic { flex: none; padding-top: 1px; margin-right: 10px; }
  .main { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
  .line1 { display: flex; align-items: center; gap: 6px; min-height: 20px; }
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
  .meta-right { display: inline-flex; align-items: center; gap: 6px; flex: none; }
  .quick { display: none; align-items: center; gap: 1px; flex: none; margin: -2px -4px -2px 0; }
  .row:hover .meta-right, .row:focus-within .meta-right { display: none; }
  .row:hover .quick, .row:focus-within .quick { display: inline-flex; }
  .qbtn {
    display: inline-grid;
    place-items: center;
    width: 24px;
    height: 24px;
    border: 0;
    border-radius: 6px;
    background: transparent;
    color: var(--muted);
    transition: background 0.1s, color 0.1s;
  }
  .qbtn:hover { background: var(--active); color: var(--text); }

  .date { flex: none; font-size: 12px; color: var(--muted); font-variant-numeric: tabular-nums; }
  .flag { color: var(--flag); display: grid; }
  .clip { color: var(--muted); display: grid; }
  .subject, .snippet { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .subject { font-size: 13px; color: var(--text-2); }
  .unread .subject { color: var(--text); font-weight: 500; }
  .snippet { font-size: 12.5px; color: var(--muted); display: flex; align-items: center; gap: 8px; }
  .snip { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  /* Quiet right-side account indicator using a vertical bar + label — never looks like a round unread dot. */
  .acc-tag {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 4px;
    max-width: 82px;
    font-size: 11px;
    font-weight: 500;
    color: var(--muted);
  }
  .acc-bar { width: 3px; height: 9px; border-radius: 2px; flex: none; }
  .acc-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

  .older { position: absolute; left: 0; right: 0; height: 56px; display: flex; align-items: center; justify-content: center; }
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
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 6px;
    padding: 24px;
    text-align: center;
    color: var(--muted);
  }
  .empty-icon {
    display: grid;
    place-items: center;
    width: 40px;
    height: 40px;
    border-radius: 50%;
    background: var(--hover);
    color: var(--muted);
    margin-bottom: 4px;
  }
  .empty-title { margin: 0; font-size: 14px; font-weight: 550; color: var(--text-2); }
  .empty-sub { margin: 0; font-size: 12.5px; color: var(--muted); max-width: 240px; line-height: 1.45; }
</style>
