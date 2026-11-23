<script lang="ts">
  import { onMount, untrack } from "svelte";
  import Icon from "./Icon.svelte";
  import MailFrame from "./MailFrame.svelte";
  import { ui, compose, accountById, accountLabel, trustSender, toast, roleOf, editDraft } from "../state.svelte";
  import { api, type Addr, type Summary } from "../api";
  import { longDate, shortDate, displayName, initials, colorFor, fileSize } from "../format";
  import { hasRemoteContent, textToHtml } from "../html";

  let { m, expanded, focus = false }: { m: Summary; expanded: boolean; focus?: boolean } = $props();

  let open = $state(untrack(() => expanded));
  let showImages = $state(false);
  let error = $state<string | null>(null);
  let el: HTMLDivElement;

  const d = $derived(ui.details[m.id]);
  const body = $derived(d ? (d.html ?? textToHtml(d.text ?? "")) : "");
  const remote = $derived(!!d?.html && hasRemoteContent(d.html));
  const trusted = $derived(ui.trustedSenders.includes(m.fromAddr.toLowerCase()));
  const allowRemote = $derived(trusted || showImages);
  const isDraft = $derived(roleOf(m) === "drafts");
  const acc = $derived(accountById(m.accountId));

  $effect(() => {
    if (open && !ui.details[m.id]) {
      const id = m.id;
      api.message(id).then(
        (x) => (ui.details[id] = x),
        (e) => (error = String(e)),
      );
    }
  });

  onMount(() => {
    if (focus) el?.scrollIntoView({ block: "start" });
  });

  function names(list: Addr[]) {
    const me = new Set(ui.accounts.map((a) => a.email.toLowerCase()));
    return list.map((a) => (me.has(a.email.toLowerCase()) ? "me" : displayName(a.name, a.email))).join(", ");
  }

  async function save(index: number) {
    try {
      await api.saveAttachment(m.id, index);
    } catch (e) {
      toast(String(e));
    }
  }

  async function openFile(index: number) {
    try {
      await api.openAttachment(m.id, index);
    } catch (e) {
      toast(String(e));
    }
  }
</script>

<div class="msg" class:open bind:this={el}>
  {#if !open}
    <button class="collapsed" onclick={() => (open = true)}>
      <span class="avatar small" style:background={colorFor(m.fromAddr)}>{initials(m.fromName, m.fromAddr)}</span>
      <span class="cname" class:unread={!m.seen}>
        {#if isDraft}<span class="draft">Draft</span>{/if}
        {displayName(m.fromName, m.fromAddr)}
      </span>
      <span class="csnip">{m.snippet}</span>
      <span class="cdate">{shortDate(m.date)}</span>
    </button>
  {:else}
    <div class="meta">
      <button class="avatar" style:background={colorFor(m.fromAddr)} onclick={() => (open = false)} title="Collapse">{initials(m.fromName, m.fromAddr)}</button>
      <div class="who">
        <div class="line">
          {#if isDraft}<span class="draft">Draft</span>{/if}
          <span class="name">{displayName(m.fromName, m.fromAddr)}</span>
          {#if m.fromName}<span class="addr">{m.fromAddr}</span>{/if}
        </div>
        {#if d}
          <div class="to">
            to {names(d.to) || "undisclosed recipients"}{#if d.cc.length}, cc {names(d.cc)}{/if}{#if d.bcc.length}, bcc {names(d.bcc)}{/if}
          </div>
        {/if}
      </div>
      <div class="when" title={longDate(m.date)}>
        <div>{shortDate(m.date)}</div>
        {#if acc && ui.accounts.length > 1}
          <div class="acc" title={acc.email}><span class="dot" style:background={acc.color || "var(--muted)"}></span>{accountLabel(acc)}</div>
        {/if}
      </div>
      <div class="own">
        {#if isDraft}
          <button class="btn small" disabled={!d} onclick={() => d && editDraft(d)}><Icon name="pencil" size={13} /> Edit</button>
        {:else if ui.thread.length > 1}
          <button class="icon-btn small" title="Reply" disabled={!d} onclick={() => compose("reply", { source: d })}><Icon name="reply" size={15} /></button>
          <button class="icon-btn small" title="Reply all" disabled={!d} onclick={() => compose("replyAll", { source: d })}><Icon name="replyAll" size={15} /></button>
          <button class="icon-btn small" title="Forward" disabled={!d} onclick={() => compose("forward", { source: d })}><Icon name="forward" size={15} /></button>
        {/if}
      </div>
    </div>

    {#if d}
      {#if d.attachments.length}
        <div class="files">
          {#each d.attachments as a (a.index)}
            <div class="file">
              <button class="openf" onclick={() => openFile(a.index)} title="Open">
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
          <button class="link" onclick={() => (showImages = true)}>Show</button>
          <button class="link" onclick={() => trustSender(m.fromAddr)}>Always from this sender</button>
        </div>
      {/if}

      <div class="paper">
        <MailFrame html={body} {allowRemote} />
      </div>
    {:else if error}
      <div class="err">{error}</div>
    {:else}
      <div class="loading"><span class="spinner"></span></div>
    {/if}
  {/if}
</div>

<style>
  .msg { border-top: 1px solid var(--line); }
  .msg:first-child { border-top: 0; }
  .msg.open { padding: 16px 0 22px; }
  .collapsed {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    height: 48px;
    padding: 0 4px;
    border: 0;
    background: transparent;
    text-align: left;
    color: var(--text);
  }
  .collapsed:hover { background: var(--hover); }
  .cname { flex: none; max-width: 40%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 13.5px; color: var(--text-2); }
  .cname.unread { font-weight: 650; color: var(--text); }
  .csnip { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--muted); font-size: 13px; }
  .cdate { flex: none; color: var(--muted); font-size: 12px; }
  .draft { color: var(--danger); font-weight: 600; margin-right: 4px; }
  .meta { display: flex; align-items: flex-start; gap: 12px; margin-bottom: 18px; }
  .avatar {
    width: 38px;
    height: 38px;
    flex: none;
    border: 0;
    padding: 0;
    border-radius: 50%;
    display: grid;
    place-items: center;
    color: #fff;
    font-size: 13.5px;
    font-weight: 600;
    letter-spacing: 0.02em;
  }
  .avatar.small { width: 26px; height: 26px; font-size: 10.5px; }
  .who { flex: 1; min-width: 0; -webkit-user-select: text; user-select: text; }
  .line { display: flex; align-items: baseline; gap: 8px; min-width: 0; overflow: hidden; }
  .name { font-weight: 600; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; flex: none; max-width: 100%; }
  .addr { color: var(--muted); font-size: 13px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .to { color: var(--muted); font-size: 13px; margin-top: 2px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .when { flex: none; text-align: right; color: var(--muted); font-size: 12.5px; white-space: nowrap; padding-top: 2px; }
  .acc { display: flex; align-items: center; justify-content: flex-end; gap: 6px; margin-top: 3px; }
  .dot { width: 6px; height: 6px; border-radius: 50%; }
  .own { flex: none; display: flex; gap: 0; margin: -4px -6px 0 0; }
  .icon-btn.small { width: 28px; height: 28px; }
  .btn.small { height: 28px; padding: 0 10px; display: inline-flex; align-items: center; gap: 5px; font-size: 12.5px; }
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
  .openf {
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
  .err { color: var(--muted); font-size: 13px; padding: 8px 0; }
  .loading { display: grid; place-items: center; height: 80px; }
  .spinner {
    width: 18px;
    height: 18px;
    border-radius: 50%;
    border: 2px solid var(--line-strong);
    border-top-color: var(--muted);
    animation: spin 0.8s linear infinite;
  }
</style>
