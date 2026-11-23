<script lang="ts">
  import Icon from "./Icon.svelte";
  import { ui, setView, compose, openSettings, openAccountSettings, accountLabel, reorderAccounts, reorderFolders, unifiedRoles } from "../state.svelte";
  import { sortable } from "../sortable";
  import { accountMenu, folderMenu } from "../menus";
  import { hint } from "../shortcuts.svelte";
  import { api, type Folder, type MailView } from "../api";

  const roleIcon: Record<string, string> = { inbox: "inbox", snoozed: "clock", sent: "sent", drafts: "drafts", archive: "archive", trash: "trash", junk: "junk" };
  const hiddenNames = new Set(["outbox", "templates", "snoozed"]);

  let collapsed = $state<Record<number, boolean>>({});
  let allCollapsed = $state(false);
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

  /** Inbox or a custom folder of this account has unread mail (drafts, sent, junk, etc. don't count). */
  const hasUnread = (accountId: number) => ui.folders.some((f) => f.accountId === accountId && showCount(f));

  /** All Accounts: each standard folder that at least one account has, with unread summed across accounts. */
  const unified = $derived(
    unifiedRoles
      .map((r) => {
        const fs = ui.folders.filter((f) => f.role === r.role);
        return { ...r, present: fs.length > 0, unread: fs.reduce((n, f) => n + f.unread, 0) };
      })
      .filter((r) => r.present),
  );
  /** Mail waiting for a later time, or that failed. Messages just sent (still undoable) don't count. */
  const scheduled = $derived(ui.outbox.filter((o) => o.error || o.sendAt * 1000 > Date.now() + (ui.undoSend + 5) * 1000));
  const syncing = $derived(Object.values(ui.status).some((s) => s.state === "syncing"));
  const errors = $derived(Object.entries(ui.status).filter(([, s]) => s.state === "error"));

  // While searching, the scope is what's highlighted.
  const searching = $derived(!!ui.query.trim());
  const isActive = (f: Folder) =>
    searching ? ui.searchScope.kind === "folder" && ui.searchScope.folderId === f.id : ui.view.kind === "folder" && ui.view.folderId === f.id;
  const isUnifiedActive = (role: string) =>
    searching ? ui.searchScope.kind === "unified" && ui.searchScope.role === role : ui.view.kind === "unified" && ui.view.role === role;
  const isAccountScope = (id: number) => searching && ui.searchScope.kind === "account" && ui.searchScope.accountId === id;

  function openFolderMenu(e: MouseEvent, view: MailView, unread: number) {
    e.preventDefault();
    folderMenu(view, unread);
  }
  const showCount = (f: Folder) => f.unread > 0 && (f.role === "inbox" || !f.role);
</script>

<aside class="side">
  <header data-tauri-drag-region>
    <button class="icon-btn" title={hint("New message", "compose")} onclick={() => compose()}>
      <Icon name="compose" size={17} />
    </button>
  </header>

  <nav class="scroll">
    {#if ui.accounts.length}
      <div class="group all">
        <div class="head-row">
          <button class="group-head" onclick={() => (allCollapsed = !allCollapsed)}>
            <span class="glyph"><Icon name="inboxes" size={12} stroke={2} /></span>
            <span class="label">All Accounts</span>
            <span class="chev" class:open={!allCollapsed}><Icon name="chevron" size={12} stroke={2} /></span>
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
              {#if u.role === "inbox" && (scheduled.length || ui.view.kind === "outbox")}
                <button class="item" class:active={ui.view.kind === "outbox" && !ui.query} onclick={() => setView({ kind: "outbox" })}>
                  <Icon name="outbox" />
                  <span class="label">Scheduled</span>
                  <span class="count" class:failed={scheduled.some((o) => o.error)}>{scheduled.length || ""}</span>
                </button>
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
        <div class="group" data-id={acc.id}>
          <div class="head-row">
            <button
              class="group-head"
              class:scoped={isAccountScope(acc.id)}
              title={acc.email}
              onclick={() => (collapsed[acc.id] = !collapsed[acc.id])}
              oncontextmenu={(e) => { e.preventDefault(); accountMenu(acc, !!collapsed[acc.id], () => (collapsed[acc.id] = !collapsed[acc.id])); }}>
              <span class="dot" style:background={acc.color || "var(--muted)"}></span>
              <span class="label">{accountLabel(acc)}</span>
              {#if st?.state === "error"}
                <span class="warn" title={st.message ?? ""}><Icon name="alert" size={13} /></span>
              {/if}
              {#if hasUnread(acc.id)}<span class="unread" title="Unread mail"></span>{/if}
              <span class="chev" class:open={!collapsed[acc.id]}><Icon name="chevron" size={12} stroke={2} /></span>
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
              {/each}
            </div>
            {#if fs.extra}
              <button class="more" onclick={() => (showMore[acc.id] = !showMore[acc.id])}>
                {showMore[acc.id] ? "Less" : "More"}
                {#if fs.hiddenUnread}<span class="unread" title="Unread mail in hidden folders"></span>{/if}
              </button>
            {/if}
          {/if}
        </div>
      {/each}
    </div>

    {#if !ui.accounts.length}
      <button class="add" onclick={() => openAccountSettings("new")}>
        <Icon name="plus" /> Add account
      </button>
    {/if}
  </nav>

  <footer>
    <button class="icon-btn" title={hint("Settings", "settings")} onclick={() => openSettings()}>
      <Icon name="settings" />
    </button>
    <button class="icon-btn" title="Accounts" onclick={() => openAccountSettings()}>
      <Icon name="at" />
    </button>
    <span class="sync" title={errors.length ? `${errors.length} account(s) can't connect` : ""}>
      {#if errors.length && !syncing}
        <span class="err"><Icon name="alert" size={13} /></span>
      {/if}
    </span>
    <button class="icon-btn" title={syncing ? "Checking for mail…" : hint("Check for mail", "sync")} onclick={() => api.syncNow()}>
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
    padding: 0 12px;
  }
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
  .count { font-size: 12px; font-variant-numeric: tabular-nums; color: var(--muted); font-weight: 500; }
  .item.active .count { color: var(--text-2); }
  .count.failed { color: var(--danger); }
  .group { padding-top: 6px; background: var(--side); border-radius: 8px; }
  .group:global([data-dragging]), .item:global([data-dragging]) { box-shadow: 0 6px 20px rgba(20, 18, 14, 0.14); }
  .item:global([data-dragging]) { background: var(--panel); }
  .group:global([data-dragging]) .group-head { background: var(--hover); }
  .head-row { position: relative; }

  .group-head { font-size: 12px; font-weight: 600; color: var(--muted); height: 28px; }
  .group-head.scoped { background: var(--active); color: var(--text); }
  .group.all { margin-bottom: 8px; }
  .glyph { display: grid; place-items: center; width: 16px; flex: none; color: var(--muted); }
  .dot { width: 8px; height: 8px; border-radius: 50%; flex: none; margin: 0 4px 0 4px; }
  .chev { display: grid; color: var(--muted); opacity: 0; transition: transform 0.15s, opacity 0.12s; }
  .head-row:hover .chev { opacity: 1; }
  .chev.open { transform: rotate(90deg); }
  .warn { color: var(--danger); display: grid; }
  .unread { width: 6px; height: 6px; border-radius: 50%; flex: none; background: var(--accent); }
  .more .unread { margin-left: -3px; }
  .more { font-size: 12px; color: var(--muted); padding-left: 34px; height: 26px; }
  .add { color: var(--accent); font-weight: 500; }
  footer {
    flex: none;
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 10px 12px;
  }
  .sync { flex: 1; display: flex; justify-content: center; color: var(--muted); }
  footer .icon-btn > span { display: grid; }
  .spin { animation: spin 1s linear infinite; }
  .err { display: grid; color: var(--danger); }
</style>
