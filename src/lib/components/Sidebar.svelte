<script lang="ts">
  import Icon from "./Icon.svelte";
  import { ui, setView, compose, openSettings, accountLabel, reorderAccounts, reorderFolders } from "../state.svelte";
  import { sortable } from "../sortable";
  import { hint } from "../shortcuts.svelte";
  import { api, type Folder } from "../api";

  const roleIcon: Record<string, string> = { inbox: "inbox", sent: "sent", drafts: "drafts", archive: "archive", trash: "trash", junk: "junk" };
  const hiddenNames = new Set(["outbox", "templates", "snoozed"]);

  let collapsed = $state<Record<number, boolean>>({});
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
    return { visible, extra };
  }

  const totalUnread = $derived(ui.folders.filter((f) => f.role === "inbox").reduce((n, f) => n + f.unread, 0));
  const syncing = $derived(Object.values(ui.status).some((s) => s.state === "syncing"));
  const errors = $derived(Object.entries(ui.status).filter(([, s]) => s.state === "error"));

  const isActive = (f: Folder) => ui.view.kind === "folder" && ui.view.folderId === f.id && !ui.query;
  const showCount = (f: Folder) => f.unread > 0 && (f.role === "inbox" || !f.role);
</script>

<aside class="side">
  <header data-tauri-drag-region>
    <button class="icon-btn" title={hint("New message", "compose")} onclick={() => compose()}>
      <Icon name="compose" size={17} />
    </button>
  </header>

  <nav class="scroll">
    <button
      class="item top"
      class:active={ui.view.kind === "unified" && !ui.query}
      onclick={() => setView({ kind: "unified" })}>
      <Icon name="inboxes" />
      <span class="label">Inbox</span>
      {#if totalUnread}<span class="count">{totalUnread}</span>{/if}
    </button>

    <div class="accounts" use:sortable={{ item: ".group", handle: ".group-head", onsort: (ids) => reorderAccounts(ids.map(Number)) }}>
      {#each ui.accounts as acc (acc.id)}
        {@const fs = foldersFor(acc.id)}
        {@const st = ui.status[acc.id]}
        <div class="group" data-id={acc.id}>
          <div class="head-row">
            <button
              class="group-head"
              title={acc.email}
              onclick={() => (collapsed[acc.id] = !collapsed[acc.id])}
              oncontextmenu={(e) => { e.preventDefault(); openSettings(acc.id); }}>
              <span class="dot" style:background={acc.color || "var(--muted)"}></span>
              <span class="label">{accountLabel(acc)}</span>
              {#if st?.state === "error"}
                <span class="warn" title={st.message ?? ""}><Icon name="alert" size={13} /></span>
              {/if}
              <span class="chev" class:open={!collapsed[acc.id]}><Icon name="chevron" size={12} stroke={2} /></span>
            </button>
            <button class="edit" title="Edit account" onclick={() => openSettings(acc.id)}><Icon name="pencil" size={12} stroke={2} /></button>
          </div>
          {#if !collapsed[acc.id]}
            <div class="folders" use:sortable={{ item: ".item", onsort: (ids) => reorderFolders(acc.id, ids.map(Number)) }}>
              {#each fs.visible as f (f.id)}
                <button class="item" data-id={f.id} class:active={isActive(f)} onclick={() => setView({ kind: "folder", folderId: f.id })}>
                  <Icon name={roleIcon[f.role ?? ""] ?? "folder"} />
                  <span class="label">{f.name}</span>
                  {#if showCount(f)}<span class="count">{f.unread}</span>{/if}
                </button>
              {/each}
            </div>
            {#if fs.extra}
              <button class="more" onclick={() => (showMore[acc.id] = !showMore[acc.id])}>
                {showMore[acc.id] ? "Less" : "More"}
              </button>
            {/if}
          {/if}
        </div>
      {/each}
    </div>

    {#if !ui.accounts.length}
      <button class="add" onclick={() => openSettings("new")}>
        <Icon name="plus" /> Add account
      </button>
    {/if}
  </nav>

  <footer>
    <button class="icon-btn" title={hint("Settings", "settings")} onclick={() => openSettings()}>
      <Icon name="settings" />
    </button>
    <span class="sync" title={errors.length ? `${errors.length} account(s) can't connect` : ""}>
      {#if syncing}
        <span class="spin"><Icon name="refresh" size={13} /></span>
      {:else if errors.length}
        <span class="err"><Icon name="alert" size={13} /></span>
      {/if}
    </span>
    <button class="icon-btn" title={hint("Check for mail", "sync")} onclick={() => api.syncNow()}>
      <Icon name="refresh" size={15} />
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
  .item.top { margin-bottom: 10px; color: var(--text); font-weight: 500; }
  .item :global(svg) { flex: none; color: var(--muted); }
  .item.active :global(svg), .item.top :global(svg) { color: var(--text); }
  .label { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .count { font-size: 12px; font-variant-numeric: tabular-nums; color: var(--muted); font-weight: 500; }
  .item.active .count, .item.top .count { color: var(--text-2); }
  .group { padding-top: 6px; background: var(--side); border-radius: 8px; }
  .group:global([data-dragging]), .item:global([data-dragging]) { box-shadow: 0 6px 20px rgba(20, 18, 14, 0.14); }
  .item:global([data-dragging]) { background: var(--panel); }
  .group:global([data-dragging]) .group-head { background: var(--hover); }
  .head-row { position: relative; }
  .head-row .group-head { padding-right: 30px; }
  .edit {
    position: absolute;
    right: 4px;
    top: 4px;
    display: grid;
    place-items: center;
    width: 20px;
    height: 20px;
    padding: 0;
    border: 0;
    border-radius: 5px;
    background: transparent;
    color: var(--muted);
    opacity: 0;
    transition: opacity 0.12s;
  }
  .head-row:hover .edit { opacity: 1; }
  .edit:hover { background: var(--active); color: var(--text); }
  .group-head { font-size: 12px; font-weight: 600; color: var(--muted); height: 28px; }
  .dot { width: 8px; height: 8px; border-radius: 50%; flex: none; margin: 0 4px 0 4px; }
  .chev { display: grid; color: var(--muted); opacity: 0; transition: transform 0.15s, opacity 0.12s; }
  .head-row:hover .chev { opacity: 1; }
  .chev.open { transform: rotate(90deg); }
  .warn { color: var(--danger); display: grid; }
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
  .spin { display: grid; animation: spin 1s linear infinite; }
  .err { display: grid; color: var(--danger); }
</style>
