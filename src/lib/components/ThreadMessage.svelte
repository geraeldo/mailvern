<script lang="ts">
  import { onMount, untrack } from "svelte";
  import Icon from "./Icon.svelte";
  import MailFrame from "./MailFrame.svelte";
  import CalendarCard from "./CalendarCard.svelte";
  import { ui, compose, accountById, accountLabel, isSenderTrusted, shouldStripTrackers, trustSender, trustDomain, toast, roleOf, editDraft, myEmails } from "../state.svelte";
  import { api, type Addr, type Summary } from "../api";
  import { longDate, shortDate, displayName, fileSize } from "../format";
  import Avatar from "./Avatar.svelte";
  import { addressMenu } from "../menus";
  import { hasRemoteContent, textToHtml } from "../html";
  import { trustableDomain } from "../trust";
  import { cachedPhishingWarnings, getCachedDetail, hasShownImages, markShownImages, putDetail } from "../mail-cache.svelte";

  let {
    m,
    expanded,
    focus = false,
    collapsible = false,
    active = true,
  }: { m: Summary; expanded: boolean; focus?: boolean; collapsible?: boolean; active?: boolean } = $props();

  let open = $state(untrack(() => expanded));
  let error = $state<string | null>(null);
  let el: HTMLDivElement;

  const d = $derived(getCachedDetail(m.id));
  const isPlain = $derived(!!d && !d.html);
  const body = $derived(d ? (d.html ?? textToHtml(d.text ?? "")) : "");
  const remote = $derived(!!d?.html && hasRemoteContent(d.html));
  const domain = $derived(trustableDomain(m.fromAddr));
  const trusted = $derived(isSenderTrusted(m.fromAddr));
  const allowRemote = $derived(trusted || hasShownImages(m.id));
  const stripTrackers = $derived(shouldStripTrackers(m.fromAddr));
  const isDraft = $derived(roleOf(m) === "drafts");
  const isSentOrDraft = $derived(roleOf(m) === "sent" || isDraft);
  const warnings = $derived.by(() => {
    if (!d || isSentOrDraft) return [];
    const header = d.warnings ?? [];
    const links = cachedPhishingWarnings(m.id, d.html, stripTrackers);
    return [...header, ...links];
  });
  const acc = $derived(accountById(m.accountId));

  $effect(() => {
    if (active && open && !getCachedDetail(m.id)) {
      const id = m.id;
      let cancelled = false;
      error = null;
      api.message(id).then(
        (x) => {
          putDetail(x);
          if (!cancelled && ui.thread.some((t) => t.id === id)) ui.details[id] = x;
        },
        (e) => {
          if (!cancelled) error = String(e);
        },
      );
      return () => {
        cancelled = true;
      };
    }
  });

  onMount(() => {
    if (focus) el?.scrollIntoView({ block: "start" });
  });

  const me = $derived(myEmails());
  const sender = $derived<Addr>({ name: m.fromName, email: m.fromAddr });

  // Clicking non-text header areas folds the message back up; clicking selectable sender/recipient
  // text or action buttons won't accidentally collapse while trying to copy an address.
  function headerClick(e: MouseEvent) {
    if (!collapsible) return;
    if ((e.target as HTMLElement).closest("button, a, .who")) return;
    if (window.getSelection()?.toString()) return;
    open = false;
  }

  let stagedPaths = $state<Record<number, string>>({});

  async function save(index: number) {
    try {
      await api.saveAttachment(m.id, index);
    } catch (e) {
      toast(String(e));
    }
  }

  async function saveAll() {
    try {
      const n = await api.saveAllAttachments(m.id, null);
      if (n > 0) toast(n === 1 ? "Saved 1 attachment to Downloads" : `Saved ${n} attachments to Downloads`);
    } catch (e) {
      toast(String(e));
    }
  }

  async function quickLook(index: number) {
    try {
      await api.quicklookAttachment(m.id, index);
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

  function warmStage(index: number) {
    if (stagedPaths[index]) return;
    api.stageAttachment(m.id, index).then((info) => {
      stagedPaths[index] = info.path;
    }).catch(() => {});
  }

  function onDragAttachment(e: DragEvent, index: number, filename: string, mime: string) {
    const p = stagedPaths[index];
    if (!p || !e.dataTransfer) return;
    const fileUrl = `file://${encodeURI(p)}`;
    e.dataTransfer.setData("text/uri-list", fileUrl);
    e.dataTransfer.setData("DownloadURL", `${mime || "application/octet-stream"}:${filename}:${fileUrl}`);
    e.dataTransfer.setData("application/x-mailvern-file", p);
    e.dataTransfer.effectAllowed = "copy";
  }
</script>

{#snippet names(list: Addr[])}
  {#each list as a, i}{#if i}, {/if}<span class="who-addr" role="presentation" title={a.email} oncontextmenu={(e) => addressMenu(e, a, d)}>{me.has(a.email.toLowerCase()) ? "me" : displayName(a.name, a.email)}</span>{/each}
{/snippet}

<div class="msg" class:open bind:this={el}>
  {#if !open}
    <button type="button" class="collapsed" onclick={() => (open = true)}>
      <Avatar name={m.fromName} email={m.fromAddr} size={26} lookup={roleOf(m) !== "junk"} />
      <span class="cname" class:unread={!m.seen}>
        {#if isDraft}<span class="draft">Draft</span>{/if}
        {displayName(m.fromName, m.fromAddr)}
      </span>
      <span class="csnip">{m.snippet}</span>
      {#if m.flagged}<span class="cflag"><Icon name="flag" size={12} stroke={2} /></span>{/if}
      {#if m.hasAttachments}<span class="cclip"><Icon name="clip" size={12} stroke={2} /></span>{/if}
      <span class="cdate">{shortDate(m.date)}</span>
    </button>
  {:else}
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <div class="meta" class:collapsible role="presentation" onclick={headerClick}>
      <button type="button" class="avatar" disabled={!collapsible} onclick={() => (open = false)} title={collapsible ? "Collapse message" : undefined}>
        <Avatar name={m.fromName} email={m.fromAddr} size={38} lookup={roleOf(m) !== "junk"} />
      </button>
      <div class="who">
        <div class="line">
          {#if isDraft}<span class="draft">Draft</span>{/if}
          <span class="name" role="presentation" oncontextmenu={(e) => addressMenu(e, sender, d, true)}>{displayName(m.fromName, m.fromAddr)}</span>
          {#if m.fromName}<span class="addr" role="presentation" oncontextmenu={(e) => addressMenu(e, sender, d, true)}>{m.fromAddr}</span>{/if}
        </div>
        {#if d}
          <div class="to">
            to {#if d.to.length}{@render names(d.to)}{:else}undisclosed recipients{/if}{#if d.cc.length}, cc {@render names(d.cc)}{/if}{#if d.bcc.length}, bcc {@render names(d.bcc)}{/if}
          </div>
        {/if}
      </div>
      <div class="when" title={longDate(m.date)}>
        <div>{shortDate(m.date)}</div>
        {#if acc && ui.accounts.length > 1}
          <div class="acc" title={acc.email}><span class="acc-bar" style:background={acc.color || "var(--muted)"}></span>{accountLabel(acc)}</div>
        {/if}
      </div>
      <div class="own">
        {#if isDraft}
          <button type="button" class="btn sm" disabled={!d} onclick={() => d && editDraft(d)}><Icon name="pencil" size={13} /> Edit</button>
        {:else if collapsible}
          <button type="button" class="icon-btn small" title="Reply" aria-label="Reply" disabled={!d} onclick={() => compose("reply", { source: d })}><Icon name="reply" size={15} /></button>
          <button type="button" class="icon-btn small" title="Reply all" aria-label="Reply all" disabled={!d} onclick={() => compose("replyAll", { source: d })}><Icon name="replyAll" size={15} /></button>
          <button type="button" class="icon-btn small" title="Forward" aria-label="Forward" disabled={!d} onclick={() => compose("forward", { source: d })}><Icon name="forward" size={15} /></button>
          <button type="button" class="icon-btn small fold-btn" title="Collapse message" aria-label="Collapse message" onclick={() => (open = false)}><Icon name="chevronDown" size={14} /></button>
        {/if}
      </div>
    </div>

    {#if d}
      {#if warnings.length}
        {@const severe = warnings.some((w) => w.severity === "danger")}
        <div class="phish" class:severe role="alert">
          <Icon name="shieldAlert" size={16} />
          <div class="phish-body">
            {#each warnings as w (w.kind)}
              <div class="phish-item">
                <strong>{w.title}.</strong> {w.detail}
              </div>
            {/each}
          </div>
        </div>
      {/if}

      {#if d.calendarInvite}
        <CalendarCard messageId={m.id} invite={d.calendarInvite} />
      {/if}

      {#if d.attachments.length}
        <div class="files">
          {#each d.attachments as a (a.index)}
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div
              class="file"
              draggable="true"
              onpointerdown={() => warmStage(a.index)}
              ondragstart={(e) => onDragAttachment(e, a.index, a.filename, a.mime)}>
              <button type="button" class="openf" onclick={() => openFile(a.index)} title={`Open ${a.filename}`}>
                <span class="ficon"><Icon name="file" size={16} /></span>
                <span class="fname">{a.filename}</span>
                <span class="fsize">{fileSize(a.size)}</span>
              </button>
              <button type="button" class="icon-btn dl" title="Quick Look preview" aria-label={`Quick Look ${a.filename}`} onclick={() => quickLook(a.index)}>
                <Icon name="eye" size={14} />
              </button>
              <button type="button" class="icon-btn dl" title="Save to Downloads" aria-label={`Download ${a.filename}`} onclick={() => save(a.index)}>
                <Icon name="download" size={15} />
              </button>
            </div>
          {/each}
          {#if d.attachments.length > 1}
            <button type="button" class="btn sm save-all" onclick={saveAll}>
              <Icon name="download" size={13} /> Save all ({d.attachments.length})
            </button>
          {/if}
        </div>
      {/if}

      {#if remote && !allowRemote}
        <div class="images">
          <Icon name="image" size={15} />
          <span>Images hidden</span>
          <button type="button" class="link" onclick={() => markShownImages(m.id)}>Show once</button>
          <button type="button" class="link" title={`Always load remote images from ${m.fromAddr}`} onclick={() => trustSender(m.fromAddr)}>Always from {displayName(m.fromName, m.fromAddr)}</button>
          {#if domain}
            <button type="button" class="link" title={`Always load remote images from anyone at ${domain}`} onclick={() => trustDomain(domain)}>Always from @{domain}</button>
          {/if}
        </div>
      {/if}

      <div class="paper" class:plain={isPlain}>
        <MailFrame html={body} {allowRemote} plain={isPlain} {stripTrackers} cacheKey={m.id} {active} />
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
    width: calc(100% + 16px);
    margin: 4px -8px;
    height: 44px;
    padding: 0 10px;
    border: 0;
    border-radius: 8px;
    background: transparent;
    text-align: left;
    color: var(--text);
  }
  .collapsed:hover { background: var(--hover); }
  .cname { flex: none; max-width: 38%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 13.5px; color: var(--text-2); }
  .cname.unread { font-weight: 650; color: var(--text); }
  .csnip { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--muted); font-size: 13px; }
  .cflag { color: var(--flag); display: grid; flex: none; }
  .cclip { color: var(--muted); display: grid; flex: none; }
  .cdate { flex: none; color: var(--muted); font-size: 12px; font-variant-numeric: tabular-nums; }
  .draft { color: var(--danger); font-weight: 600; margin-right: 4px; }
  .meta { display: flex; align-items: flex-start; gap: 12px; margin-bottom: 18px; }
  .avatar { flex: none; border: 0; padding: 0; background: none; border-radius: 50%; display: grid; }
  .avatar:disabled { cursor: default; opacity: 1; }
  .who { flex: 1; min-width: 80px; -webkit-user-select: text; user-select: text; }
  .line { display: flex; align-items: baseline; gap: 8px; min-width: 0; overflow: hidden; }
  .name { font-weight: 600; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; flex: none; max-width: 100%; }
  .addr { color: var(--muted); font-size: 13px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .to { color: var(--muted); font-size: 13px; margin-top: 2px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .when { flex: none; text-align: right; color: var(--muted); font-size: 12.5px; white-space: nowrap; padding-top: 2px; font-variant-numeric: tabular-nums; }
  .acc { display: flex; align-items: center; justify-content: flex-end; gap: 5px; margin-top: 3px; font-size: 11.5px; }
  .acc-bar { width: 3px; height: 9px; border-radius: 2px; flex: none; }
  .own { flex: none; display: flex; gap: 0; margin: -4px -6px 0 0; }
  .icon-btn.small { width: 28px; height: 28px; }
  .fold-btn :global(svg) { transform: rotate(180deg); }
  @container (max-width: 520px) {
    .meta { gap: 10px; }
    .own .icon-btn.small { width: 24px; }
  }
  .files { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; margin-bottom: 20px; }
  .save-all { height: 34px; }
  .phish {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 10px 13px;
    margin-bottom: 14px;
    border-radius: 9px;
    border: 1px solid color-mix(in srgb, var(--warn) 35%, transparent);
    background: color-mix(in srgb, var(--warn) 10%, transparent);
    color: var(--text);
    font-size: 12.5px;
    line-height: 1.45;
  }
  .phish :global(svg) { flex: none; margin-top: 2px; color: var(--warn); }
  .phish.severe {
    border-color: color-mix(in srgb, var(--danger) 35%, transparent);
    background: var(--danger-soft);
  }
  .phish.severe :global(svg) { color: var(--danger); }
  .phish-body { display: flex; flex-direction: column; gap: 4px; min-width: 0; }
  .phish-item strong { font-weight: 600; }
  .file {
    display: flex;
    align-items: center;
    border: 1px solid var(--line-strong);
    border-radius: 9px;
    max-width: min(300px, 100%);
    overflow: hidden;
    transition: border-color 0.12s;
  }
  .file:hover { border-color: var(--muted); }
  .openf {
    flex: 1;
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
  .openf:hover { background: var(--hover); }
  .ficon { color: var(--muted); display: grid; flex: none; }
  .fname { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 13px; font-weight: 500; }
  .fsize { flex: none; color: var(--muted); font-size: 12px; white-space: nowrap; }
  .dl { flex: none; margin: 0 4px 0 2px; width: 28px; height: 28px; }
  .images {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 10px;
    row-gap: 4px;
    margin-bottom: 16px;
    padding: 8px 12px;
    border-radius: 8px;
    background: var(--hover);
    color: var(--muted);
    font-size: 13px;
  }
  .images span { flex: 1; white-space: nowrap; }
  .link { border: 0; background: none; padding: 0; color: var(--accent); font-size: 13px; font-weight: 500; white-space: nowrap; cursor: pointer; }
  .link:hover { text-decoration: underline; }
  .paper { border-radius: 10px; overflow: hidden; }
  @media (prefers-color-scheme: dark) {
    .paper:not(.plain) { padding: 16px 18px; background: #fff; }
  }
  .err { color: var(--danger); font-size: 13px; padding: 8px 0; }
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
