<script lang="ts">
  import Icon from "./Icon.svelte";
  import MailFrame from "./MailFrame.svelte";
  import { ui, compose, archiveSelected, trashSelected, toggleSeen, toggleFlag, accountById, accountLabel, trustSender, toast } from "../state.svelte";
  import { hint } from "../shortcuts.svelte";
  import { api, type Addr } from "../api";
  import { longDate, displayName, initials, colorFor, fileSize } from "../format";
  import { hasRemoteContent, textToHtml } from "../html";

  let showImagesFor = $state<number | null>(null);

  const d = $derived(ui.detail);
  const body = $derived(d ? (d.html ?? textToHtml(d.text ?? "")) : "");
  const remote = $derived(!!d?.html && hasRemoteContent(d.html));
  const trusted = $derived(!!d && ui.trustedSenders.includes(d.fromAddr.toLowerCase()));
  const allowRemote = $derived(!!d && (trusted || showImagesFor === d.id));
  const row = $derived(ui.list.find((m) => m.id === ui.selectedId));

  function names(list: Addr[]) {
    const me = new Set(ui.accounts.map((a) => a.email.toLowerCase()));
    return list.map((a) => (me.has(a.email.toLowerCase()) ? "me" : displayName(a.name, a.email))).join(", ");
  }

  async function save(index: number) {
    if (!d) return;
    try {
      await api.saveAttachment(d.id, index);
    } catch (e) {
      toast(String(e));
    }
  }

  async function open(index: number) {
    if (!d) return;
    try {
      await api.openAttachment(d.id, index);
    } catch (e) {
      toast(String(e));
    }
  }
</script>

<section class="reader">
  <header data-tauri-drag-region>
    {#if d}
      <div class="tools">
        <button class="icon-btn" title={hint("Reply", "reply")} onclick={() => compose("reply")}><Icon name="reply" size={17} /></button>
        <button class="icon-btn" title={hint("Reply all", "replyAll")} onclick={() => compose("replyAll")}><Icon name="replyAll" size={17} /></button>
        <button class="icon-btn" title={hint("Forward", "forward")} onclick={() => compose("forward")}><Icon name="forward" size={17} /></button>
        <span class="sep"></span>
        <button class="icon-btn" title={hint("Archive", "archive")} onclick={archiveSelected}><Icon name="archive" size={17} /></button>
        <button class="icon-btn" title={hint("Delete", "delete")} onclick={trashSelected}><Icon name="trash" size={17} /></button>
        <span class="sep"></span>
        <button class="icon-btn" class:on={row && !row.seen} title={hint("Mark as unread", "toggleUnread")} onclick={toggleSeen}><Icon name="unread" size={17} /></button>
        <button class="icon-btn" class:flagged={row?.flagged} title={hint("Flag", "toggleFlag")} onclick={toggleFlag}><Icon name="flag" size={17} /></button>
      </div>
    {/if}
  </header>

  {#if d}
    {@const acc = accountById(d.accountId)}
    <div class="scroll content">
      <article>
        <h2>{d.subject || "(no subject)"}</h2>
        <div class="meta">
          <span class="avatar" style:background={colorFor(d.fromAddr)}>{initials(d.fromName, d.fromAddr)}</span>
          <div class="who">
            <div class="line">
              <span class="name">{displayName(d.fromName, d.fromAddr)}</span>
              {#if d.fromName}<span class="addr">{d.fromAddr}</span>{/if}
            </div>
            <div class="to">
              to {names(d.to) || "undisclosed recipients"}{#if d.cc.length}, cc {names(d.cc)}{/if}
            </div>
          </div>
          <div class="when">
            <div>{longDate(d.date)}</div>
            {#if acc && ui.accounts.length > 1}
              <div class="acc" title={acc.email}><span class="dot" style:background={acc.color || "var(--muted)"}></span>{accountLabel(acc)}</div>
            {/if}
          </div>
        </div>

        {#if d.attachments.length}
          <div class="files">
            {#each d.attachments as a (a.index)}
              <div class="file">
                <button class="open" onclick={() => open(a.index)} title="Open">
                  <span class="ficon"><Icon name="file" size={16} /></span>
                  <span class="fname">{a.filename}</span>
                  <span class="fsize">{fileSize(a.size)}</span>
                </button>
                <button class="icon-btn dl" title="Save to Downloads" onclick={() => save(a.index)}>
                  <Icon name="download" size={15} />
                </button>
              </div>
            {/each}
          </div>
        {/if}

        {#if remote && !allowRemote}
          <div class="images">
            <Icon name="image" size={15} />
            <span>Images hidden</span>
            <button class="link" onclick={() => (showImagesFor = d.id)}>Show</button>
            <button class="link" onclick={() => trustSender(d.fromAddr)}>Always from this sender</button>
          </div>
        {/if}

        <div class="paper">
          {#key d.id}
            <MailFrame html={body} {allowRemote} />
          {/key}
        </div>
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
  }
  header {
    height: 52px;
    flex: none;
    display: flex;
    align-items: center;
    justify-content: flex-end;
    padding: 0 14px;
  }
  .tools { display: flex; align-items: center; gap: 2px; }
  .sep { width: 1px; height: 18px; background: var(--line); margin: 0 6px; }
  .icon-btn.flagged { color: #e0882a; }
  .content { flex: 1; }
  article { max-width: 860px; margin: 0 auto; padding: 14px 36px 48px; animation: fade 0.12s ease-out; }
  h2 {
    margin: 0 0 20px;
    font-size: 22px;
    line-height: 1.25;
    font-weight: 650;
    letter-spacing: -0.015em;
    -webkit-user-select: text;
    user-select: text;
  }
  .meta { display: flex; align-items: flex-start; gap: 12px; margin-bottom: 22px; }
  .avatar {
    width: 38px;
    height: 38px;
    flex: none;
    border-radius: 50%;
    display: grid;
    place-items: center;
    color: #fff;
    font-size: 13.5px;
    font-weight: 600;
    letter-spacing: 0.02em;
  }
  .who { flex: 1; min-width: 0; -webkit-user-select: text; user-select: text; }
  .line { display: flex; align-items: baseline; gap: 8px; min-width: 0; }
  .name { font-weight: 600; white-space: nowrap; }
  .addr { color: var(--muted); font-size: 13px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .to { color: var(--muted); font-size: 13px; margin-top: 2px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .when { flex: none; text-align: right; color: var(--muted); font-size: 12.5px; }
  .acc { display: flex; align-items: center; justify-content: flex-end; gap: 6px; margin-top: 3px; }
  .dot { width: 6px; height: 6px; border-radius: 50%; }
  .files { display: flex; flex-wrap: wrap; gap: 8px; margin-bottom: 20px; }
  .file {
    display: flex;
    align-items: center;
    border: 1px solid var(--line-strong);
    border-radius: 9px;
    max-width: 300px;
    overflow: hidden;
  }
  .file:hover { border-color: var(--muted); }
  .open {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    height: 38px;
    padding: 0 4px 0 11px;
    border: 0;
    background: transparent;
    text-align: left;
  }
  .ficon { color: var(--muted); display: grid; }
  .fname { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 13px; font-weight: 500; }
  .fsize { color: var(--muted); font-size: 12px; white-space: nowrap; }
  .dl { margin-right: 4px; width: 28px; height: 28px; }
  .images {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 16px;
    padding: 8px 12px;
    border-radius: 8px;
    background: var(--hover);
    color: var(--muted);
    font-size: 13px;
  }
  .images span { flex: 1; }
  .link { border: 0; background: none; padding: 0; color: var(--accent); font-size: 13px; font-weight: 500; }
  .paper { border-radius: 10px; overflow: hidden; }
  @media (prefers-color-scheme: dark) {
    .paper { padding: 16px 18px; background: #fff; }
  }
  .center { flex: 1; display: grid; place-items: center; color: var(--muted); }
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
