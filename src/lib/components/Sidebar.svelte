<script lang="ts">
  import Icon from "./Icon.svelte";
  import {
    ui, setView, compose, openSettings, openAccountSettings, accountLabel,
    reorderAccounts, reorderFolders, reorderSavedSearches, openSavedSearch, unifiedRoles, toggleSidebar,
  } from "../state.svelte";
  import { sortable } from "../sortable";
  import { accountMenu, allAccountsMenu, folderMenu, savedSearchMenu } from "../menus";
  import { hint } from "../shortcuts.svelte";
  import { api, type Folder, type MailView } from "../api";

  const roleIcon: Record<string, string> = { inbox: "inbox", snoozed: "clock", sent: "sent", drafts: "drafts", archive: "archive", trash: "trash", junk: "junk" };
  const hiddenNames = new Set(["outbox", "templates", "snoozed"]);

  let collapsed = $state<Record<number, boolean>>({});
  let allCollapsed = $state(false);
  let savedCollapsed = $state(false);
  let showMore = $state<Record<number, boolean>>({});

  /**
   * Folders in the saved order. Custom folders below the last standard folder sit
   * behind "More"; drag one above it to keep it visible.
   */
  function foldersFor(accountId: number) {
    const all = ui.folders.filter((f) => f.accountId === accountId && !hiddenNames.has(f.name.toLowerCase()));
    const isMain = (f: Folder) => !!f.role && f.role !== "all";
    let cut = 0;
    all.forEach((f, i) => isMain(f) && (cut = i + 1));
    const extra = all.length - cut;
    const visible = showMore[accountId] ? all : all.slice(0, cut);
    const hiddenUnread = !showMore[accountId] && all.slice(cut).some(showCount);
    return { visible, extra, hiddenUnread };
  }

  const showCount = (f: Folder) => f.unread > 0 && (f.role === "inbox" || !f.role);

  /** Total unread count for an account (shown subtly when collapsed instead of a dot). */
  const accountUnread = (accountId: number) =>
    ui.folders.filter((f) => f.accountId === accountId && showCount(f)).reduce((n, f) => n + f.unread, 0);

  /** All Accounts: each standard folder that at least one account has, with unread summed across accounts. */
  const unified = $derived(
    unifiedRoles
      .map((r) => {
        const fs = ui.folders.filter((f) => f.role === r.role);
        return { ...r, present: fs.length > 0, unread: fs.reduce((n, f) => n + f.unread, 0) };
      })
      .filter((r) => r.present),
  );
  const showUnified = $derived(ui.accounts.length > 1);
  /** Mail waiting for a later time, or that failed. Messages just sent (still undoable) don't count. */
  const scheduled = $derived(ui.outbox.filter((o) => o.error || o.sendAt * 1000 > Date.now() + (ui.undoSend + 5) * 1000));
  const syncing = $derived(Object.values(ui.status).some((s) => s.state === "syncing"));
  const errors = $derived(Object.entries(ui.status).filter(([, s]) => s.state === "error"));

  // While searching, the scope is what's highlighted.
  const searching = $derived(!!ui.query.trim());
  const isActive = (f: Folder) =>
    searching
      ? ui.searchScope.kind === "folder" && ui.searchScope.folderId === f.id
      : (ui.view.kind === "folder" && ui.view.folderId === f.id) ||
        (!showUnified && ui.view.kind === "unified" && f.role === ui.view.role);
  const isUnifiedActive = (role: string) =>
    searching ? ui.searchScope.kind === "unified" && ui.searchScope.role === role : ui.view.kind === "unified" && ui.view.role === role;
  const isSmartActive = (name: "flagged" | "unread") => !searching && ui.view.kind === "smart" && ui.view.name === name;
  const isAccountScope = (id: number) => searching && ui.searchScope.kind === "account" && ui.searchScope.accountId === id;

  const everyCollapsed = () => (!showUnified || allCollapsed) && ui.accounts.every((a) => collapsed[a.id]);
  function setAllCollapsed(value: boolean) {
    allCollapsed = value;
    collapsed = Object.fromEntries(ui.accounts.map((a) => [a.id, value]));
  }
  const fold = (isCollapsed: boolean, toggle: () => void) => ({ collapsed: isCollapsed, toggle, allCollapsed: everyCollapsed(), setAll: setAllCollapsed });

  function openFolderMenu(e: MouseEvent, view: MailView, unread: number) {
    e.preventDefault();
    folderMenu(view, unread);
  }
</script>

<aside class="side">
  <header data-tauri-drag-region>
    <button class="icon-btn" title="Hide sidebar  ⌥⌘S" aria-label="Hide sidebar" onclick={toggleSidebar}>
      <Icon name="sidebar" size={16} />
    </button>
    <button class="icon-btn compose-btn" title={hint("New message", "compose")} aria-label="New message" onclick={() => compose()}>
      <Icon name="compose" size={16} />
    </button>
  </header>

  <nav class="scroll">
    {#if showUnified}
      <div class="group all">
        <div class="head-row">
          <button
            class="group-head"
            onclick={() => (allCollapsed = !allCollapsed)}
            oncontextmenu={(e) => { e.preventDefault(); allAccountsMenu(fold(allCollapsed, () => (allCollapsed = !allCollapsed))); }}>
            <span class="glyph"><Icon name="inboxes" size={12} stroke={2} /></span>
            <span class="label">All Accounts</span>
            <span class="chev" class:open={!allCollapsed} class:closed={allCollapsed}><Icon name="chevron" size={12} stroke={2} /></span>
          </button>
        </div>
        {#if !allCollapsed}
          <div class="folders">
            {#each unified as u (u.role)}
              <button
                class="item"
                class:active={isUnifiedActive(u.role)}
                onclick={() => setView({ kind: "unified", role: u.role })}
                oncontextmenu={(e) => openFolderMenu(e, { kind: "unified", role: u.role }, u.unread)}>
                <Icon name={roleIcon[u.role]} />
                <span class="label">{u.name}</span>
                {#if u.unread && u.role === "inbox"}<span class="count">{u.unread}</span>{/if}
              </button>
              {#if u.role === "inbox"}
                <button
                  class="item"
                  class:active={isSmartActive("unread")}
                  onclick={() => setView({ kind: "smart", name: "unread" })}
                  oncontextmenu={(e) => openFolderMenu(e, { kind: "smart", name: "unread" }, ui.smartCounts.unread)}>
                  <Icon name="unread" />
                  <span class="label">Unread</span>
                  {#if ui.smartCounts.unread}<span class="count">{ui.smartCounts.unread}</span>{/if}
                </button>
                <button
                  class="item"
                  class:active={isSmartActive("flagged")}
                  onclick={() => setView({ kind: "smart", name: "flagged" })}
                  oncontextmenu={(e) => openFolderMenu(e, { kind: "smart", name: "flagged" }, 0)}>
                  <Icon name="flag" />
                  <span class="label">Flagged</span>
                  {#if ui.smartCounts.flagged}<span class="count">{ui.smartCounts.flagged}</span>{/if}
                </button>
                {#if scheduled.length || ui.view.kind === "outbox"}
                  <button class="item" class:active={ui.view.kind === "outbox" && !ui.query} onclick={() => setView({ kind: "outbox" })}>
                    <Icon name="outbox" />
                    <span class="label">Scheduled</span>
                    <span class="count" class:failed={scheduled.some((o) => o.error)}>{scheduled.length || ""}</span>
                  </button>
                {/if}
              {/if}
            {/each}
          </div>
        {/if}
      </div>
    {/if}

    <div class="accounts" use:sortable={{ item: ".group", handle: ".group-head", onsort: (ids) => reorderAccounts(ids.map(Number)) }}>
      {#each ui.accounts as acc (acc.id)}
        {@const fs = foldersFor(acc.id)}
        {@const st = ui.status[acc.id]}
        {@const unreadTotal = accountUnread(acc.id)}
        <div class="group" data-id={acc.id}>
          <div class="head-row">
            <button
              class="group-head"
              class:scoped={isAccountScope(acc.id)}
              title={acc.email}
              onclick={() => (collapsed[acc.id] = !collapsed[acc.id])}
              oncontextmenu={(e) => { e.preventDefault(); accountMenu(acc, fold(!!collapsed[acc.id], () => (collapsed[acc.id] = !collapsed[acc.id]))); }}>
              <span class="acc-bar" style:background={acc.color || "var(--muted)"}></span>
              <span class="label">{accountLabel(acc)}</span>
              {#if st?.state === "error"}
                <span class="warn" title={st.message ?? "Connection error"}><Icon name="alert" size={13} /></span>
              {/if}
              {#if collapsed[acc.id] && unreadTotal}
                <span class="count">{unreadTotal}</span>
              {/if}
              <span class="chev" class:open={!collapsed[acc.id]} class:closed={!!collapsed[acc.id]}><Icon name="chevron" size={12} stroke={2} /></span>
            </button>
          </div>
          {#if !collapsed[acc.id]}
            <div class="folders" use:sortable={{ item: ".item", onsort: (ids) => reorderFolders(acc.id, ids.map(Number)) }}>
              {#each fs.visible as f (f.id)}
                <button
                  class="item"
                  data-id={f.id}
                  class:active={isActive(f)}
                  onclick={() => setView({ kind: "folder", folderId: f.id })}
                  oncontextmenu={(e) => openFolderMenu(e, { kind: "folder", folderId: f.id }, f.unread)}>
                  <Icon name={roleIcon[f.role ?? ""] ?? "folder"} />
                  <span class="label">{f.name}</span>
                  {#if showCount(f)}<span class="count">{f.unread}</span>{/if}
                </button>
                {#if !showUnified && f.role === "inbox"}
                  <button
                    class="item"
                    class:active={isSmartActive("unread")}
                    onclick={() => setView({ kind: "smart", name: "unread" })}
                    oncontextmenu={(e) => openFolderMenu(e, { kind: "smart", name: "unread" }, ui.smartCounts.unread)}>
                    <Icon name="unread" />
                    <span class="label">Unread</span>
                    {#if ui.smartCounts.unread}<span class="count">{ui.smartCounts.unread}</span>{/if}
                  </button>
                  <button
                    class="item"
                    class:active={isSmartActive("flagged")}
                    onclick={() => setView({ kind: "smart", name: "flagged" })}
                    oncontextmenu={(e) => openFolderMenu(e, { kind: "smart", name: "flagged" }, 0)}>
                    <Icon name="flag" />
                    <span class="label">Flagged</span>
                    {#if ui.smartCounts.flagged}<span class="count">{ui.smartCounts.flagged}</span>{/if}
                  </button>
                  {#if scheduled.length || ui.view.kind === "outbox"}
                    <button class="item" class:active={ui.view.kind === "outbox" && !ui.query} onclick={() => setView({ kind: "outbox" })}>
                      <Icon name="outbox" />
                      <span class="label">Scheduled</span>
                      <span class="count" class:failed={scheduled.some((o) => o.error)}>{scheduled.length || ""}</span>
                    </button>
                  {/if}
                {/if}
              {/each}
            </div>
            {#if fs.extra}
              <button class="more" onclick={() => (showMore[acc.id] = !showMore[acc.id])}>
                {showMore[acc.id] ? "Less" : "More"}
                {#if fs.hiddenUnread}<span class="more-unread" title="Unread mail in hidden folders"></span>{/if}
              </button>
            {/if}
          {/if}
        </div>
      {/each}
    </div>

    {#if ui.savedSearches.length}
      <div class="group saved">
        <div class="head-row">
          <button class="group-head" onclick={() => (savedCollapsed = !savedCollapsed)}>
            <span class="glyph"><Icon name="bookmark" size={12} stroke={2} /></span>
            <span class="label">Saved Searches</span>
            <span class="chev" class:open={!savedCollapsed} class:closed={savedCollapsed}><Icon name="chevron" size={12} stroke={2} /></span>
          </button>
        </div>
        {#if !savedCollapsed}
          <div class="folders" use:sortable={{ item: ".item", onsort: (ids) => reorderSavedSearches(ids.map(Number)) }}>
            {#each ui.savedSearches as s (s.id)}
              <button
                class="item"
                data-id={s.id}
                class:active={searching && ui.activeSavedSearchId === s.id}
                title={s.query}
                onclick={() => openSavedSearch(s)}
                oncontextmenu={(e) => { e.preventDefault(); savedSearchMenu(s); }}>
                <Icon name="search" size={14} />
                <span class="label">{s.name}</span>
                {#if s.unread > 0}<span class="count">{s.unread}</span>{/if}
              </button>
            {/each}
          </div>
        {/if}
      </div>
    {/if}

    {#if !ui.accounts.length}
      <button class="add" onclick={() => openAccountSettings("new")}>
        <Icon name="plus" /> Add account
      </button>
    {/if}
  </nav>

  <footer>
    <button class="icon-btn" title={hint("Settings", "settings")} aria-label="Settings" onclick={() => openSettings()}>
      <Icon name="settings" size={16} />
    </button>
    <button class="icon-btn" title="Keyboard shortcuts  ?" aria-label="Keyboard shortcuts" onclick={() => (ui.shortcutHelp = true)}>
      <Icon name="help" size={16} />
    </button>
    <span class="sync">
      {#if errors.length && !syncing}
        <button
          class="err-pill"
          title={errors[0][1].message || `${errors.length} account(s) can't connect — click to fix`}
          onclick={() => openAccountSettings(Number(errors[0][0]))}>
          <Icon name="alert" size={12} />
          <span>{errors.length > 1 ? `${errors.length} errors` : "Sync error"}</span>
        </button>
      {:else if ui.pendingAgentActions > 0}
        <button
          class="warn-pill"
          title="Agent actions waiting for approval — click to review"
          onclick={() => openSettings("agents")}>
          <Icon name="code" size={12} />
          <span>{ui.pendingAgentActions === 1 ? "1 approval" : `${ui.pendingAgentActions} approvals`}</span>
        </button>
      {/if}
    </span>
    <button class="icon-btn" title={syncing ? "Checking for mail…" : hint("Check for mail", "sync")} aria-label="Check for mail" onclick={() => api.syncNow()}>
      <span class:spin={syncing}><Icon name="refresh" size={15} /></span>
    </button>
  </footer>
</aside>

<style>
  .side {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: var(--side);
    border-right: 1px solid var(--line);
    min-width: 0;
  }
  header {
    height: 52px;
    flex: none;
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 2px;
    padding: 0 10px 0 82px;
  }
  .compose-btn { color: var(--text); }
  nav { flex: 1; padding: 2px 10px 12px; }
  .item, .group-head, .more, .add {
    display: flex;
    align-items: center;
    gap: 9px;
    width: 100%;
    height: 30px;
    padding: 0 9px;
    border: 0;
    border-radius: 7px;
    background: transparent;
    color: var(--text-2);
    text-align: left;
    font-size: 13.5px;
  }
  .item:hover, .group-head:hover, .more:hover, .add:hover { background: var(--hover); }
  .item.active { background: var(--active); color: var(--text); font-weight: 500; }
  .item :global(svg) { flex: none; color: var(--muted); }
  .item.active :global(svg) { color: var(--text); }
  .label { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .count { font-size: 12px; font-variant-numeric: tabular-nums; color: var(--muted); font-weight: 500; flex: none; }
  .item.active .count { color: var(--text-2); }
  .count.failed { color: var(--danger); }
  .group { padding-top: 6px; background: var(--side); border-radius: 8px; }
  .group:first-child { padding-top: 0; }
  .group:global([data-dragging]), .item:global([data-dragging]) { box-shadow: 0 6px 20px rgba(20, 18, 14, 0.14); }
  .item:global([data-dragging]) { background: var(--panel); }
  .group:global([data-dragging]) .group-head { background: var(--hover); }
  .head-row { position: relative; }

  .group-head { font-size: 12px; font-weight: 600; color: var(--muted); height: 28px; }
  .group-head.scoped { background: var(--active); color: var(--text); }
  .group.all { margin-bottom: 8px; }
  .group.saved { margin-top: 8px; }
  .glyph { display: grid; place-items: center; width: 16px; flex: none; color: var(--muted); }
  /* Vertical bar instead of a round dot so account colors never resemble unread dots. */
  .acc-bar { width: 3px; height: 12px; border-radius: 2px; flex: none; margin: 0 6px 0 7px; }
  .chev { display: grid; color: var(--muted); opacity: 0; transition: transform 0.15s, opacity 0.12s; flex: none; }
  .chev.closed { opacity: 0.7; }
  .head-row:hover .chev { opacity: 1; }
  .chev.open { transform: rotate(90deg); }
  .warn { color: var(--danger); display: grid; flex: none; }
  .more-unread { width: 6px; height: 6px; border-radius: 50%; flex: none; background: var(--accent); margin-left: -3px; }
  .more { font-size: 12px; color: var(--muted); padding-left: 34px; height: 26px; }
  .add { color: var(--accent); font-weight: 500; }
  footer {
    flex: none;
    display: flex;
    align-items: center;
    gap: 2px;
    padding: 8px 10px;
    border-top: 1px solid var(--line);
  }
  .sync { flex: 1; min-width: 0; display: flex; justify-content: center; color: var(--muted); }
  .err-pill, .warn-pill {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    max-width: 100%;
    height: 24px;
    padding: 0 8px;
    border: 0;
    border-radius: 99px;
    font-size: 11.5px;
    font-weight: 550;
  }
  .err-pill { background: var(--danger-soft); color: var(--danger); }
  .warn-pill { background: var(--warn-soft); color: var(--warn-text); }
  .err-pill:hover, .warn-pill:hover { filter: brightness(0.95); }
  .err-pill span, .warn-pill span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  footer .icon-btn > span { display: grid; }
  .spin { animation: spin 1s linear infinite; }
</style>
