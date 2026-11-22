<script lang="ts">
  import Icon from "./Icon.svelte";
  import { ui, setView, compose } from "../state.svelte";
  import { api, type Folder } from "../api";

  const roleOrder: Record<string, number> = { inbox: 0, drafts: 1, sent: 2, archive: 3, junk: 4, trash: 5 };
  const roleIcon: Record<string, string> = { inbox: "inbox", sent: "sent", drafts: "drafts", archive: "archive", trash: "trash", junk: "junk" };
  const hiddenNames = new Set(["outbox", "templates", "snoozed"]);

  let collapsed = $state<Record<number, boolean>>({});
  let showMore = $state<Record<number, boolean>>({});

  function foldersFor(accountId: number) {
    const all = ui.folders.filter((f) => f.accountId === accountId);
    const main = all
      .filter((f) => f.role && f.role !== "all")
      .sort((a, b) => (roleOrder[a.role!] ?? 9) - (roleOrder[b.role!] ?? 9));
    const other = all.filter((f) => !f.role && !hiddenNames.has(f.name.toLowerCase()));
    return { main, other };
  }

  const totalUnread = $derived(ui.folders.filter((f) => f.role === "inbox").reduce((n, f) => n + f.unread, 0));
  const syncing = $derived(Object.values(ui.status).some((s) => s.state === "syncing"));
  const errors = $derived(Object.entries(ui.status).filter(([, s]) => s.state === "error"));

  const isActive = (f: Folder) => ui.view.kind === "folder" && ui.view.folderId === f.id && !ui.query;
  const showCount = (f: Folder) => f.unread > 0 && (f.role === "inbox" || !f.role);
</script>

<aside class="side">
  <header data-tauri-drag-region>
    <button class="icon-btn" title="New message  ⌘N" onclick={() => compose()}>
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

    {#each ui.accounts as acc (acc.id)}
      {@const fs = foldersFor(acc.id)}
      {@const st = ui.status[acc.id]}
      <div class="group">
        <button class="group-head" onclick={() => (collapsed[acc.id] = !collapsed[acc.id])}>
          <span class="dot" style:background={acc.color || "var(--muted)"}></span>
          <span class="label">{acc.name || acc.email}</span>
          {#if st?.state === "error"}
            <span class="warn" title={st.message ?? ""}><Icon name="alert" size={13} /></span>
          {/if}
          <span class="chev" class:open={!collapsed[acc.id]}><Icon name="chevron" size={12} stroke={2} /></span>
        </button>
        {#if !collapsed[acc.id]}
          {#each fs.main as f (f.id)}
            <button class="item" class:active={isActive(f)} onclick={() => setView({ kind: "folder", folderId: f.id })}>
              <Icon name={roleIcon[f.role!] ?? "folder"} />
              <span class="label">{f.name}</span>
              {#if showCount(f)}<span class="count">{f.unread}</span>{/if}
            </button>
          {/each}
          {#if fs.other.length}
            {#if showMore[acc.id]}
              {#each fs.other as f (f.id)}
                <button class="item" class:active={isActive(f)} onclick={() => setView({ kind: "folder", folderId: f.id })}>
                  <Icon name="folder" />
                  <span class="label">{f.name}</span>
                  {#if showCount(f)}<span class="count">{f.unread}</span>{/if}
                </button>
              {/each}
            {/if}
            <button class="more" onclick={() => (showMore[acc.id] = !showMore[acc.id])}>
              {showMore[acc.id] ? "Less" : "More"}
            </button>
          {/if}
        {/if}
      </div>
    {/each}

    {#if !ui.accounts.length}
      <button class="add" onclick={() => (ui.settings = true)}>
        <Icon name="plus" /> Add account
      </button>
    {/if}
  </nav>

  <footer>
    <button class="icon-btn" title="Settings  ⌘," onclick={() => (ui.settings = true)}>
      <Icon name="settings" />
    </button>
    <span class="sync" title={errors.length ? `${errors.length} account(s) can't connect` : ""}>
      {#if syncing}
        <span class="spin"><Icon name="refresh" size={13} /></span>
      {:else if errors.length}
        <span class="err"><Icon name="alert" size={13} /></span>
      {/if}
    </span>
    <button class="icon-btn" title="Check for mail  ⌘R" onclick={() => api.syncNow()}>
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
  .group { margin-top: 6px; }
  .group-head { font-size: 12px; font-weight: 600; color: var(--muted); height: 28px; }
  .dot { width: 8px; height: 8px; border-radius: 50%; flex: none; margin: 0 4px 0 4px; }
  .chev { display: grid; color: var(--muted); opacity: 0; transition: transform 0.15s, opacity 0.12s; }
  .group-head:hover .chev { opacity: 1; }
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
