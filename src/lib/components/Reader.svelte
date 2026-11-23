<script lang="ts">
  import Icon from "./Icon.svelte";
  import MailFrame from "./MailFrame.svelte";
  import ThreadMessage from "./ThreadMessage.svelte";
  import {
    ui, compose, archiveSelected, trashSelected, toggleJunk, toggleSeen, toggleFlag, selectedRows, roleOf, clearSelection,
    snoozeSelected, unsnoozeSelected, moveSelected, accountById, editQueued, deleteQueued, sendQueuedNow, copyThread,
  } from "../state.svelte";
  import { hint } from "../shortcuts.svelte";
  import { whenLabel } from "../when";

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
  /** Scroll to the first unread message of a longer conversation, else stay at the top. */
  const focusId = $derived(ui.thread.length > 3 ? (ui.thread.find((m) => ui.expandIds.includes(m.id) && !m.seen)?.id ?? null) : null);
</script>

<section class="reader">
  <header data-tauri-drag-region>
    {#if outboxItem}
      <div class="tools">
        <button class="btn" onclick={() => sendQueuedNow(outboxItem.id)}>Send now</button>
        <button class="btn" onclick={() => editQueued(outboxItem.id)}>Edit</button>
        <button class="icon-btn" title="Delete" onclick={() => deleteQueued(outboxItem.id)}><Icon name="trash" size={17} /></button>
      </div>
    {:else if showTools}
      <div class="tools">
        {#if !multi}
          <button class="icon-btn" title={hint("Reply", "reply")} onclick={() => compose("reply")}><Icon name="reply" size={17} /></button>
          <button class="icon-btn" title={hint("Reply all", "replyAll")} onclick={() => compose("replyAll")}><Icon name="replyAll" size={17} /></button>
          <button class="icon-btn" title={hint("Forward", "forward")} onclick={() => compose("forward")}><Icon name="forward" size={17} /></button>
          <span class="sep"></span>
        {/if}
        <button class="icon-btn" title={hint("Archive", "archive")} onclick={archiveSelected}><Icon name="archive" size={17} /></button>
        <button class="icon-btn" title={hint("Delete", "delete")} onclick={trashSelected}><Icon name="trash" size={17} /></button>
        {#if isJunk}
          <button class="icon-btn" title={hint("Not junk", "junk")} onclick={toggleJunk}><Icon name="inbox" size={17} /></button>
        {:else}
          <button class="icon-btn" title={hint("Move to junk", "junk")} onclick={toggleJunk}><Icon name="junk" size={17} /></button>
        {/if}
        <button class="icon-btn" title={hint("Move to folder", "move")} disabled={!oneAccount} onclick={moveSelected}><Icon name="move" size={17} /></button>
        {#if snoozed}
          <button class="icon-btn on" title="Unsnooze" onclick={unsnoozeSelected}><Icon name="clock" size={17} /></button>
        {:else}
          <button class="icon-btn" title={hint("Snooze", "snooze")} onclick={snoozeSelected}><Icon name="clock" size={17} /></button>
        {/if}
        <span class="sep"></span>
        <button class="icon-btn" class:on={unread} title={hint(unread ? "Mark as read" : "Mark as unread", "toggleUnread")} onclick={toggleSeen}><Icon name="unread" size={17} /></button>
        <button class="icon-btn" class:flagged title={hint("Flag", "toggleFlag")} onclick={toggleFlag}><Icon name="flag" size={17} /></button>
        {#if !multi}
          <span class="sep"></span>
          <button class="icon-btn" title={hint(ui.thread.length > 1 ? "Copy conversation as text" : "Copy message as text", "copyThread")} onclick={copyThread}><Icon name="copy" size={17} /></button>
        {/if}
      </div>
    {/if}
  </header>

  {#if outboxItem}
    {@const o = outboxItem}
    {@const acc = accountById(o.accountId)}
    <div class="scroll content">
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
  {:else if multi}
    <div class="center bulk">
      <div class="big">{ui.selectedIds.length}</div>
      <div>selected</div>
      <button class="link" onclick={clearSelection}>Clear selection</button>
    </div>
  {:else if d}
    <div class="scroll content">
      <article>
        <h2>{d.subject || "(no subject)"}</h2>
        {#key ui.selectedId}
          {#each ui.thread as m, i (m.id)}
            <ThreadMessage {m} expanded={ui.expandIds.includes(m.id) || i === ui.thread.length - 1} focus={m.id === focusId} />
          {/each}
        {/key}
      </article>
    </div>
  {:else if ui.detailLoading}
    <div class="center"><span class="spinner"></span></div>
  {:else if ui.detailError}
    <div class="center error">{ui.detailError}</div>
  {:else}
    <div class="center blank" data-tauri-drag-region>
      <svg width="56" height="56" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1" stroke-linecap="round" stroke-linejoin="round"><rect width="20" height="16" x="2" y="4" rx="2"/><path d="m22 7-8.97 5.7a1.94 1.94 0 0 1-2.06 0L2 7"/></svg>
    </div>
  {/if}
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
  .tools .btn { margin-left: 6px; }
  .sep { width: 1px; height: 18px; background: var(--line); margin: 0 6px; }
  .icon-btn.flagged { color: #e0882a; }
  .icon-btn:disabled { opacity: 0.35; }
  .content { flex: 1; }
  article { max-width: 860px; margin: 0 auto; padding: 14px 36px 48px; animation: fade 0.12s ease-out; }
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
  .queued.failed { color: var(--danger); }
  .rcpt { display: flex; flex-direction: column; gap: 4px; margin-bottom: 18px; font-size: 13px; color: var(--text-2); -webkit-user-select: text; user-select: text; }
  .k { display: inline-block; width: 44px; color: var(--muted); }
  .paper { border-radius: 10px; overflow: hidden; }
  @media (prefers-color-scheme: dark) {
    .paper { padding: 16px 18px; background: #fff; }
  }
  /* A narrow window: tighter toolbar and margins so nothing spills into the list. */
  @container (max-width: 520px) {
    header { padding: 0 8px; }
    .tools { gap: 0; }
    .tools .icon-btn { width: 27px; }
    .sep { margin: 0 3px; }
    article { padding: 12px 20px 40px; }
  }
  .center { flex: 1; display: grid; place-items: center; color: var(--muted); }
  .bulk { display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 4px; font-size: 14px; }
  .big { font-size: 40px; font-weight: 650; color: var(--text-2); font-variant-numeric: tabular-nums; }
  .link { margin-top: 10px; border: 0; background: none; padding: 0; color: var(--accent); font-size: 13px; font-weight: 500; }
  .blank { color: var(--line-strong); }
  .error { padding: 40px; text-align: center; font-size: 13px; }
  .spinner {
    width: 20px;
    height: 20px;
    border-radius: 50%;
    border: 2px solid var(--line-strong);
    border-top-color: var(--muted);
    animation: spin 0.8s linear infinite;
  }
</style>
