<script lang="ts">
  import Icon from "./Icon.svelte";
  import MailFrame from "./MailFrame.svelte";
  import ThreadMessage from "./ThreadMessage.svelte";
  import {
    ui, compose, archiveSelected, trashSelected, toggleJunk, toggleSeen, toggleFlag, selectedRows, roleOf, clearSelection,
    snoozeSelected, unsnoozeSelected, moveSelected, accountById, editQueued, deleteQueued, sendQueuedNow, copyThread, openMessage,
    printOpenThread,
  } from "../state.svelte";
  import { hint, keymap, display } from "../shortcuts.svelte";
  import { whenLabel } from "../when";
  import { readerPool, type ReaderPane } from "../mail-cache.svelte";
  import { overlayScroll } from "../scrollbar";

  let aiAnswerPromise: Promise<typeof import("./AiAnswer.svelte").default> | null = null;
  const loadAiAnswer = () => (aiAnswerPromise ??= import("./AiAnswer.svelte").then((m) => m.default));
  const closeAi = () => import("../ai/search.svelte").then((m) => m.exitAi());

  const multi = $derived(ui.selectedIds.length > 1);
  const d = $derived(ui.detail);
  const outboxItem = $derived(ui.view.kind === "outbox" ? ui.outbox.find((o) => o.id === ui.outboxSelected) : undefined);
  /** The rows the toolbar acts on. */
  const rows = $derived(selectedRows());
  const isJunk = $derived(rows.length > 0 && roleOf(rows[0]) === "junk");
  const snoozed = $derived(rows.length > 0 && rows.every((r) => roleOf(r) === "snoozed"));
  const unread = $derived(rows.some((r) => !r.seen));
  const flagged = $derived(rows.length > 0 && rows.every((r) => r.flagged));
  const oneAccount = $derived(new Set(rows.map((r) => r.accountId)).size === 1);
  const showTools = $derived(ui.view.kind !== "outbox" && (multi || !!d));
  /** AI search: its answer fills the reader until a source is opened. */
  const answer = $derived(ui.ai && ui.view.kind !== "outbox" && ui.selectedId == null && !multi);
  const showEmail = $derived(!outboxItem && !(answer && ui.ai) && !multi && !!d);

  const keyFor = (action: keyof typeof keymap.bindings) => {
    const first = keymap.bindings[action][0];
    return first ? display(first) : "";
  };

  function focusIdOf(pane: ReaderPane): number | null {
    return pane.thread.length > 3 ? (pane.thread.find((m) => pane.expandIds.includes(m.id) && !m.seen)?.id ?? null) : null;
  }

  /** Preserves scroll position across display:none hide/show transitions in the keep-alive pool. */
  function keepScroll(node: HTMLElement, params: { pane: ReaderPane; active: boolean }) {
    let current = params;
    const onScroll = () => {
      if (current.active) current.pane.scrollTop = node.scrollTop;
    };
    node.addEventListener("scroll", onScroll, { passive: true });
    return {
      update(next: { pane: ReaderPane; active: boolean }) {
        const becameActive = next.active && !current.active;
        current = next;
        if (becameActive && node.scrollTop !== next.pane.scrollTop) {
          node.scrollTop = next.pane.scrollTop;
        }
      },
      destroy() {
        node.removeEventListener("scroll", onScroll);
      },
    };
  }
</script>

<section class="reader">
  <header data-tauri-drag-region>
    {#if ui.ai && !answer}
      <button type="button" class="back" title="Back to the answer  Esc" onclick={() => (multi ? clearSelection() : openMessage(null))}>
        <Icon name="arrowLeft" size={14} /> Answer
      </button>
    {/if}
    {#if answer}
      <button type="button" class="btn ghost sm" title="Close answer  Esc" onclick={closeAi}>
        <Icon name="x" size={14} /> Close answer
      </button>
    {:else if outboxItem}
      <div class="tools">
        <button type="button" class="btn primary sm" onclick={() => sendQueuedNow(outboxItem.id)}>Send now</button>
        <button type="button" class="btn sm" onclick={() => editQueued(outboxItem.id)}>Edit</button>
        <button type="button" class="icon-btn" title="Delete" aria-label="Delete" onclick={() => deleteQueued(outboxItem.id)}><Icon name="trash" size={17} /></button>
      </div>
    {:else if showTools}
      <div class="tools">
        {#if !multi}
          <button type="button" class="icon-btn" title={hint("Reply", "reply")} aria-label="Reply" onclick={() => compose("reply")}><Icon name="reply" size={17} /></button>
          <button type="button" class="icon-btn" title={hint("Reply all", "replyAll")} aria-label="Reply all" onclick={() => compose("replyAll")}><Icon name="replyAll" size={17} /></button>
          <button type="button" class="icon-btn" title={hint("Forward", "forward")} aria-label="Forward" onclick={() => compose("forward")}><Icon name="forward" size={17} /></button>
          <span class="sep"></span>
        {/if}
        <button type="button" class="icon-btn" title={hint("Archive", "archive")} aria-label="Archive" onclick={archiveSelected}><Icon name="archive" size={17} /></button>
        <button type="button" class="icon-btn" title={hint("Delete", "delete")} aria-label="Delete" onclick={trashSelected}><Icon name="trash" size={17} /></button>
        {#if isJunk}
          <button type="button" class="icon-btn" title={hint("Not junk", "junk")} aria-label="Not junk" onclick={toggleJunk}><Icon name="inbox" size={17} /></button>
        {:else}
          <button type="button" class="icon-btn" title={hint("Move to junk", "junk")} aria-label="Move to junk" onclick={toggleJunk}><Icon name="junk" size={17} /></button>
        {/if}
        <button
          type="button"
          class="icon-btn"
          title={oneAccount ? hint("Move to folder", "move") : "Select messages from one account to move"}
          aria-label="Move to folder"
          disabled={!oneAccount}
          onclick={moveSelected}>
          <Icon name="move" size={17} />
        </button>
        {#if snoozed}
          <button type="button" class="icon-btn on" title="Unsnooze" aria-label="Unsnooze" onclick={unsnoozeSelected}><Icon name="clock" size={17} /></button>
        {:else}
          <button type="button" class="icon-btn" title={hint("Snooze", "snooze")} aria-label="Snooze" onclick={snoozeSelected}><Icon name="clock" size={17} /></button>
        {/if}
        <span class="sep"></span>
        <button type="button" class="icon-btn" class:on={unread} title={hint(unread ? "Mark as read" : "Mark as unread", "toggleUnread")} aria-label={unread ? "Mark as read" : "Mark as unread"} onclick={toggleSeen}><Icon name="unread" size={17} /></button>
        <button type="button" class="icon-btn" class:flagged title={hint("Flag", "toggleFlag")} aria-label="Flag" onclick={toggleFlag}><Icon name="flag" size={17} /></button>
        {#if !multi}
          <span class="sep"></span>
          <button type="button" class="icon-btn" title={hint(ui.thread.length > 1 ? "Copy conversation as text" : "Copy message as text", "copyThread")} aria-label="Copy as text" onclick={copyThread}><Icon name="copy" size={17} /></button>
          <button type="button" class="icon-btn" title="Print  ⌘P" aria-label="Print" onclick={printOpenThread}><Icon name="print" size={17} /></button>
        {/if}
      </div>
    {/if}
  </header>

  {#if outboxItem}
    {@const o = outboxItem}
    {@const acc = accountById(o.accountId)}
    <div class="col-scroll-wrap">
      <div class="col-scroll content" use:overlayScroll>
      <article>
        <h2>{o.message.subject || "(no subject)"}</h2>
        <div class="queued" class:failed={!!o.error}>
          <Icon name={o.error ? "alert" : "clock"} size={15} />
          {#if o.error}
            <span>Not sent yet, trying again {whenLabel(new Date(o.sendAt * 1000))}. {o.error}</span>
          {:else}
            <span>Sends {whenLabel(new Date(o.sendAt * 1000))}{acc ? ` from ${acc.email}` : ""}</span>
          {/if}
        </div>
        <div class="rcpt">
          <div><span class="k">To</span>{o.message.to.join(", ") || "—"}</div>
          {#if o.message.cc.length}<div><span class="k">Cc</span>{o.message.cc.join(", ")}</div>{/if}
          {#if o.message.bcc.length}<div><span class="k">Bcc</span>{o.message.bcc.join(", ")}</div>{/if}
          {#if o.message.attachments.length}<div><span class="k">Files</span>{o.message.attachments.map((p) => p.split("/").pop()).join(", ")}</div>{/if}
        </div>
        <div class="paper">
          {#key o.id}
            <MailFrame html={o.message.html} allowRemote={false} />
          {/key}
        </div>
      </article>
      </div>
    </div>
  {:else if answer && ui.ai}
    {#await loadAiAnswer() then AiAnswerComp}
      <AiAnswerComp s={ui.ai} />
    {/await}
  {:else if multi}
    <div class="center bulk">
      <div class="big">{ui.selectedIds.length}</div>
      <div class="bulk-lbl">conversations selected</div>
      <div class="bulk-actions">
        <button type="button" class="btn sm" onclick={archiveSelected}><Icon name="archive" size={14} /> Archive</button>
        <button type="button" class="btn sm" onclick={trashSelected}><Icon name="trash" size={14} /> Delete</button>
        <button type="button" class="btn sm" onclick={toggleSeen}><Icon name="unread" size={14} /> {unread ? "Mark read" : "Mark unread"}</button>
        <button type="button" class="btn sm" disabled={!oneAccount} title={oneAccount ? "" : "Select messages from one account to move"} onclick={moveSelected}><Icon name="move" size={14} /> Move…</button>
        {#if snoozed}
          <button type="button" class="btn sm" onclick={unsnoozeSelected}><Icon name="clock" size={14} /> Unsnooze</button>
        {:else}
          <button type="button" class="btn sm" onclick={snoozeSelected}><Icon name="clock" size={14} /> Snooze…</button>
        {/if}
      </div>
      <button type="button" class="link" onclick={clearSelection}>Clear selection</button>
    </div>
  {:else if !showEmail && ui.detailLoading}
    <div class="center"><span class="spinner"></span></div>
  {:else if !showEmail && ui.detailError}
    <div class="center error">{ui.detailError}</div>
  {:else if !showEmail}
    <div class="center blank" data-tauri-drag-region>
      <div class="blank-inner">
        <svg width="46" height="46" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.15" stroke-linecap="round" stroke-linejoin="round"><rect width="20" height="16" x="2" y="4" rx="2"/><path d="m22 7-8.97 5.7a1.94 1.94 0 0 1-2.06 0L2 7"/></svg>
        <p class="blank-title">Select a message to read</p>
        <div class="blank-hints">
          <span><kbd>{keyFor("compose") || "C"}</kbd> Compose</span>
          <span><kbd>{keyFor("search") || "/"}</kbd> Search</span>
          <span><kbd>?</kbd> Shortcuts</span>
        </div>
      </div>
    </div>
  {/if}

  {#each readerPool.panes as pane (pane.id)}
    {@const active = showEmail && d?.id === pane.id}
    {@const paneIsDraft = roleOf(pane.detail) === "drafts"}
    {@const paneFocus = focusIdOf(pane)}
    <div class="col-scroll-wrap" class:hidden={!active}>
      <div class="col-scroll content" use:keepScroll={{ pane, active }} use:overlayScroll>
      <article class="pane-article">
        <h2>{pane.detail.subject || "(no subject)"}</h2>
        {#each pane.thread as m, i (m.id)}
          <ThreadMessage
            {m}
            expanded={pane.expandIds.includes(m.id) || i === pane.thread.length - 1}
            focus={m.id === paneFocus}
            collapsible={pane.thread.length > 1}
            {active} />
        {/each}
        {#if !paneIsDraft}
          <div class="reply-bar">
            <button type="button" class="btn" onclick={() => compose("reply")}>
              <Icon name="reply" size={15} />
              <span>Reply</span>
              {#if keyFor("reply")}<span class="key-hint">{keyFor("reply")}</span>{/if}
            </button>
            <button type="button" class="btn" onclick={() => compose("replyAll")}>
              <Icon name="replyAll" size={15} />
              <span>Reply all</span>
              {#if keyFor("replyAll")}<span class="key-hint">{keyFor("replyAll")}</span>{/if}
            </button>
            <button type="button" class="btn" onclick={() => compose("forward")}>
              <Icon name="forward" size={15} />
              <span>Forward</span>
              {#if keyFor("forward")}<span class="key-hint">{keyFor("forward")}</span>{/if}
            </button>
          </div>
        {/if}
      </article>
      </div>
    </div>
  {/each}
</section>

<style>
  .reader {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-width: 0;
    background: var(--panel);
    container-type: inline-size;
  }
  header {
    height: 52px;
    flex: none;
    display: flex;
    align-items: center;
    justify-content: flex-end;
    padding: 0 14px;
  }
  .tools { display: flex; align-items: center; gap: 2px; min-width: 0; }
  .back {
    margin-right: auto;
    display: inline-flex;
    align-items: center;
    gap: 5px;
    height: 28px;
    padding: 0 10px 0 6px;
    border: 0;
    border-radius: 7px;
    background: transparent;
    color: var(--accent);
    font-size: 13px;
    font-weight: 500;
  }
  .back:hover { background: var(--accent-soft); }
  .tools .btn { margin-left: 6px; }
  .sep { width: 1px; height: 18px; background: var(--line); margin: 0 6px; }
  .icon-btn.flagged { color: var(--flag); }
  .content { flex: 1; }
  article { max-width: 860px; margin: 0 auto; padding: 14px 36px 48px; animation: fade 0.12s ease-out; }
  article.pane-article { animation: none; }
  h2 {
    margin: 0 0 14px;
    font-size: 22px;
    line-height: 1.25;
    font-weight: 650;
    letter-spacing: -0.015em;
    -webkit-user-select: text;
    user-select: text;
  }
  .queued {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 14px;
    padding: 9px 12px;
    border-radius: 8px;
    background: var(--hover);
    color: var(--text-2);
    font-size: 13px;
  }
  .queued.failed { background: var(--danger-soft); color: var(--danger); }
  .rcpt { display: flex; flex-direction: column; gap: 4px; margin-bottom: 18px; font-size: 13px; color: var(--text-2); -webkit-user-select: text; user-select: text; }
  .k { display: inline-block; width: 44px; color: var(--muted); }
  .paper { border-radius: 10px; overflow: hidden; }
  @media (prefers-color-scheme: dark) {
    .paper { padding: 16px 18px; background: #fff; }
  }

  .reply-bar {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px;
    margin-top: 8px;
    padding-top: 18px;
    border-top: 1px solid var(--line);
  }
  .reply-bar .btn { color: var(--text-2); }
  .reply-bar .btn:hover { color: var(--text); }
  .key-hint {
    margin-left: 2px;
    padding: 1px 5px;
    border-radius: 4px;
    background: var(--hover);
    color: var(--muted);
    font-size: 11px;
    font-weight: 500;
  }

  /* A narrow window: tighter toolbar and margins so nothing spills into the list. */
  @container (max-width: 520px) {
    header { padding: 0 8px; }
    .tools { gap: 0; }
    .tools .icon-btn { width: 26px; }
    .sep { margin: 0 3px; }
    article { padding: 12px 20px 40px; }
  }
  .center { flex: 1; display: grid; place-items: center; color: var(--muted); }
  .bulk { display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 6px; padding: 24px; font-size: 14px; }
  .big { font-size: 42px; font-weight: 650; line-height: 1; color: var(--text); font-variant-numeric: tabular-nums; }
  .bulk-lbl { font-size: 13.5px; color: var(--muted); }
  .bulk-actions { display: flex; flex-wrap: wrap; justify-content: center; gap: 8px; margin-top: 14px; }
  .link { margin-top: 12px; border: 0; background: none; padding: 2px 4px; border-radius: 5px; color: var(--accent); font-size: 13px; font-weight: 500; cursor: pointer; }
  .link:hover { text-decoration: underline; }
  .blank { color: var(--line-strong); }
  .blank-inner { display: flex; flex-direction: column; align-items: center; gap: 10px; pointer-events: none; }
  .blank-title { margin: 2px 0 0; font-size: 13.5px; color: var(--muted); font-weight: 500; }
  .blank-hints { display: flex; align-items: center; gap: 14px; font-size: 12px; color: var(--muted); opacity: 0.85; }
  .blank-hints kbd {
    display: inline-grid;
    place-items: center;
    min-width: 19px;
    height: 19px;
    padding: 0 5px;
    margin-right: 4px;
    border-radius: 4px;
    border: 1px solid var(--line-strong);
    background: var(--side);
    color: var(--text-2);
    font: 500 11px/1 var(--font);
  }
  .error { padding: 40px; text-align: center; font-size: 13px; color: var(--danger); }
  .spinner {
    width: 20px;
    height: 20px;
    border-radius: 50%;
    border: 2px solid var(--line-strong);
    border-top-color: var(--muted);
    animation: spin 0.8s linear infinite;
  }
</style>
