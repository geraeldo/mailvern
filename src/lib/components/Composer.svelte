<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { open as openDialog, ask } from "@tauri-apps/plugin-dialog";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import Icon from "./Icon.svelte";
  import Select from "./Select.svelte";
  import Editor from "./Editor.svelte";
  import RecipientField from "./RecipientField.svelte";
  import WhenPicker from "./WhenPicker.svelte";
  import Rephrase from "./Rephrase.svelte";
  import type { RephraseContext } from "../ai/rephrase";
  import {
    ui, toast, folderById, editQueued, loadOutbox, noteSentRecipients, closeComposer, focusComposer,
    toggleMinimizeComposer, toggleExpandComposer, type ComposeSession,
  } from "../state.svelte";
  import { api, type FileInfo, type Outgoing } from "../api";
  import { sendPresets, toUnix, whenLabel } from "../when";
  import { formatAddr, fileSize } from "../format";
  import { htmlToText, prefixed, quoteForward, quoteReply, emailWrap, sanitize } from "../html";
  import { mentionsAttachment } from "../attachment-check";
  import { comboOf, match, hint } from "../shortcuts.svelte";

  let props: { session: ComposeSession } = $props();
  // The composer is keyed per open, so its setup is read once.
  const init = untrack(() => props.session);
  const minimized = $derived(props.session.minimized);
  const expanded = $derived(props.session.expanded);
  const isActive = $derived(ui.activeComposerKey === props.session.key);
  const src = init.source;
  const restore = init.restore;
  const myEmails = new Set(
    ui.accounts.flatMap((a) => [a.email.toLowerCase(), ...(a.aliases ?? []).map((x) => x.email.toLowerCase())]),
  );
  const notMe = (e: string) => !myEmails.has(e.toLowerCase());

  type IdentityOption = { value: string; accountId: number; email: string; name: string; label: string; detail: string };
  const identities = $derived.by<IdentityOption[]>(() => {
    const opts: IdentityOption[] = [];
    for (const a of ui.accounts) {
      opts.push({
        value: `${a.id}:${a.email}`,
        accountId: a.id,
        email: a.email,
        name: a.name,
        label: a.name || a.email,
        detail: a.name ? a.email : "",
      });
      for (const al of a.aliases ?? []) {
        const displayName = al.name || a.name;
        opts.push({
          value: `${a.id}:${al.email}`,
          accountId: a.id,
          email: al.email,
          name: displayName,
          label: displayName || al.email,
          detail: displayName ? al.email : "",
        });
      }
    }
    return opts;
  });

  function defaultAccount(): number {
    if (restore) return restore.message.accountId;
    if (init.accountId) return init.accountId;
    if (src) return src.accountId;
    if (ui.view.kind === "folder") {
      const f = folderById(ui.view.folderId);
      if (f) return f.accountId;
    }
    return ui.accounts[0].id;
  }

  function defaultFromEmail(accId: number): string {
    const acc = ui.accounts.find((a) => a.id === accId);
    if (!acc) return "";
    if (restore?.message.fromEmail) return restore.message.fromEmail;
    // When replying to a message sent to one of this account's aliases, default to that alias.
    if (src && (init.mode === "reply" || init.mode === "replyAll") && acc.aliases?.length) {
      const recipients = new Set([...src.to, ...src.cc].map((a) => a.email.toLowerCase()));
      const hit = acc.aliases.find((al) => recipients.has(al.email.toLowerCase()));
      if (hit) return hit.email;
    }
    return acc.email;
  }

  let initTo: string[] = [];
  let initCc: string[] = [];
  let initBcc: string[] = [];
  let initSubject = "";
  if (src) {
    const sender = src.replyTo.length ? src.replyTo : [{ name: src.fromName || null, email: src.fromAddr }];
    if (init.mode === "reply" || init.mode === "replyAll") {
      initTo = sender.map(formatAddr);
      initSubject = prefixed("Re", src.subject);
      if (init.mode === "replyAll") {
        const seen = new Set(sender.map((a) => a.email.toLowerCase()));
        const addUnique = (list: typeof src.to) =>
          list
            .filter((a) => {
              const key = a.email.toLowerCase();
              if (!notMe(a.email) || seen.has(key)) return false;
              seen.add(key);
              return true;
            })
            .map(formatAddr);
        initTo = [...initTo, ...addUnique(src.to)];
        initCc = addUnique(src.cc);
      }
    } else if (init.mode === "forward") {
      initSubject = prefixed("Fwd", src.subject);
    }
  }
  if (init.to) initTo = init.to;
  if (init.cc?.length) initCc = init.cc;
  if (init.bcc?.length) initBcc = init.bcc;
  if (init.subject) initSubject = init.subject;
  if (restore) {
    initTo = restore.message.to;
    initCc = restore.message.cc;
    initBcc = restore.message.bcc;
    initSubject = restore.message.subject;
  }
  const initAccount = defaultAccount();
  const initFromEmail = defaultFromEmail(initAccount);
  const initReplyTo =
    restore?.message.replyTo !== undefined && restore.message.replyTo !== null
      ? restore.message.replyTo
      : (ui.accounts.find((a) => a.id === initAccount)?.replyTo ?? "");
  /** Where typing starts: "To" for a fresh message, else the body. */
  const focusTo = init.mode === "new" || init.mode === "forward" || (init.mode === "draft" && !initTo.length);
  const inReplyTo = restore ? restore.message.inReplyTo : init.mode === "forward" ? null : (src?.messageId ?? null);
  const references = restore
    ? restore.message.references
    : src?.messageId
      ? [src.references, src.messageId].filter(Boolean).join(" ")
      : null;

  let accountId = $state(initAccount);
  let fromEmail = $state(initFromEmail);
  let replyTo = $state(initReplyTo);
  let showReplyTo = $state(!!initReplyTo.trim());
  let to = $state<string[]>(initTo);
  let cc = $state<string[]>(initCc);
  let bcc = $state<string[]>(initBcc);
  let showCc = $state(initCc.length > 0 || initBcc.length > 0);
  let subject = $state(initSubject);
  let files = $state<FileInfo[]>(restore?.attachments ?? init.attachments ?? []);
  let sending = $state(false);
  /** Something was typed or changed since the composer opened. */
  let dirty = $state(!!restore?.unsaved);
  let laterOpen = $state(false);

  // Drafts: saved to the account's Drafts folder a moment after each change. The draft
  // keeps one Message-ID, so each save replaces the last one.
  let messageId: string | null = restore?.message.messageId ?? null;
  /** The account whose Drafts folder holds the saved draft, if any. */
  let draftAccount: number | null = restore && !restore.unsaved ? restore.message.accountId : null;
  let unsaved = !!restore?.unsaved;
  let saving = $state(false);
  let savedAt = $state<number | null>(null);
  let saveError = "";
  let saveTimer: ReturnType<typeof setTimeout> | undefined;
  let savePromise: Promise<boolean> | null = null;
  let attachmentsLoading: Promise<void> | null = null;
  /** Sent or discarded: no more saves. */
  let finished = false;
  let editorEl = $state<HTMLDivElement>();
  let dropping = $state(false);
  let toField: RecipientField;
  let ccField = $state<RecipientField>();
  let bccField = $state<RecipientField>();

  const account = $derived(ui.accounts.find((a) => a.id === accountId)!);
  const selectedIdentity = $derived(`${accountId}:${fromEmail || account?.email || ""}`);

  function pickIdentity(val: string) {
    const hit = identities.find((o) => o.value === val);
    if (!hit) return;
    const prevAccountId = accountId;
    const prevDefaultReplyTo = ui.accounts.find((a) => a.id === prevAccountId)?.replyTo ?? "";
    accountId = hit.accountId;
    fromEmail = hit.email;
    if (hit.accountId !== prevAccountId && replyTo === prevDefaultReplyTo) {
      const nextDefault = ui.accounts.find((a) => a.id === hit.accountId)?.replyTo ?? "";
      replyTo = nextDefault;
      if (nextDefault) showReplyTo = true;
    }
  }

  function sigBlock(sig: string) {
    const clean = sig ? sanitize(sig) : "";
    return `<div data-signature="1">${clean ? `<br>${clean}` : ""}</div>`;
  }

  const signature = ui.accounts.find((a) => a.id === initAccount)?.signature ?? "";
  const quote = src ? (init.mode === "forward" ? quoteForward(src) : quoteReply(src)) : "";
  const bodySeed = init.bodyText ? `<div>${init.bodyText.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/\n/g, "<br>")}</div>` : "<div><br></div>";
  const initialHtml = restore ? restore.message.html : `${bodySeed}${sigBlock(signature)}${quote ? `<br>${quote}` : ""}`;

  onMount(() => {
    if (src && init.mode === "forward" && src.attachments.length) {
      attachmentsLoading = api
        .attachmentFiles(src.id)
        .then((f) => {
          if (finished) return;
          const have = new Set(files.map((x) => x.path));
          files = [...f.filter((x) => !have.has(x.path)), ...files];
        })
        .catch((e) => toast(String(e)))
        .finally(() => {
          attachmentsLoading = null;
        });
    }
    // Put the caret at the top of the body for replies (and drafts that have recipients); focus "To" otherwise.
    if (!focusTo) {
      queueMicrotask(() => {
        if (!editorEl) return;
        editorEl.focus();
        const r = document.createRange();
        r.setStart(editorEl.firstChild ?? editorEl, 0);
        r.collapse(true);
        const sel = window.getSelection();
        sel?.removeAllRanges();
        sel?.addRange(r);
      });
    }
    const un = getCurrentWebview().onDragDropEvent(async (e) => {
      if (props.session.minimized) return;
      // When multiple composers are open, only the active one receives OS file drops.
      const openKeys = ui.composers.filter((c) => !c.minimized).map((c) => c.key);
      const targetKey = ui.activeComposerKey != null && openKeys.includes(ui.activeComposerKey) ? ui.activeComposerKey : openKeys[openKeys.length - 1];
      if (targetKey !== props.session.key) return;
      if (e.payload.type === "over" || e.payload.type === "enter") dropping = true;
      else if (e.payload.type === "leave") dropping = false;
      else if (e.payload.type === "drop") {
        dropping = false;
        addFiles(await api.fileInfo(e.payload.paths));
      }
    });
    return () => {
      finished = true;
      clearTimeout(saveTimer);
      un.then((f) => f());
    };
  });

  // Swap the signature when the sending account changes.
  let lastAccount = initAccount;
  $effect(() => {
    if (accountId === lastAccount || !editorEl) return;
    lastAccount = accountId;
    const block = editorEl.querySelector<HTMLElement>("[data-signature]");
    const sig = account?.signature ? sanitize(account.signature) : "";
    if (block) block.innerHTML = sig ? `<br>${sig}` : "";
  });

  // Recipients and the sending account count as edits too.
  let watching = false;
  $effect(() => {
    JSON.stringify([to, cc, bcc, accountId, fromEmail, replyTo]);
    if (!watching) {
      watching = true;
      return;
    }
    untrack(changed);
  });

  function changed() {
    if (finished) return;
    dirty = true;
    unsaved = true;
    clearTimeout(saveTimer);
    saveTimer = setTimeout(saveDraft, 2500);
  }

  function outgoing(): Outgoing {
    const html = emailWrap(editorEl?.innerHTML ?? "");
    const identity = identities.find((o) => o.value === selectedIdentity);
    const isAlias = identity && account && identity.email.toLowerCase() !== account.email.toLowerCase();
    return {
      accountId,
      to,
      cc,
      bcc,
      subject: subject.trim(),
      html,
      text: htmlToText(html),
      attachments: files.map((f) => f.path),
      inReplyTo,
      references,
      messageId,
      fromEmail: isAlias ? identity.email : null,
      fromName: isAlias ? (identity.name || null) : null,
      replyTo: replyTo.trim(),
    };
  }

  /** Saves the draft now. One save runs at a time, so they can't race each other. */
  async function saveDraft(): Promise<boolean> {
    if (finished) return true;
    clearTimeout(saveTimer);
    while (savePromise) {
      await savePromise;
      if (!unsaved || finished) return true;
    }
    unsaved = false;
    saving = true;
    const out = outgoing();
    savePromise = (async () => {
      try {
        // The From account changed: the draft moves to the new account's Drafts.
        if (draftAccount != null && draftAccount !== out.accountId && messageId) {
          api.deleteDraft(draftAccount, messageId).catch(() => {});
          draftAccount = null;
        }
        messageId = await api.saveDraft(out);
        draftAccount = out.accountId;
        savedAt = Date.now();
        return true;
      } catch (e) {
        unsaved = true;
        saveError = String(e);
        return false;
      } finally {
        saving = false;
        savePromise = null;
      }
    })();
    return savePromise;
  }

  function addFiles(list: FileInfo[]) {
    const have = new Set(files.map((f) => f.path));
    files = [...files, ...list.filter((f) => !have.has(f.path))];
    changed();
  }

  async function pickFiles() {
    const picked = await openDialog({ multiple: true, directory: false });
    if (!picked) return;
    addFiles(await api.fileInfo(Array.isArray(picked) ? picked : [picked]));
  }

  /** Closing keeps your work: unsaved changes go to Drafts first. */
  async function close() {
    if (sending) return;
    if (toField?.flush() || ccField?.flush() || bccField?.flush()) changed();
    if (dirty && unsaved) {
      const ok = await saveDraft();
      if (!ok && !(await ask(`Couldn't save the draft: ${saveError}\n\nClose and lose your changes?`, { title: "Mailvern", kind: "warning", okLabel: "Close", cancelLabel: "Keep editing" }))) return;
      if (ok) toast("Saved to Drafts");
    } else if (savePromise) {
      await savePromise;
    }
    finished = true;
    clearTimeout(saveTimer);
    closeComposer(init.key);
  }

  async function discard() {
    if (sending) return;
    if ((dirty || draftAccount != null) && !(await ask("Delete this draft?", { title: "Mailvern", kind: "warning", okLabel: "Delete", cancelLabel: "Keep editing" }))) return;
    finished = true;
    clearTimeout(saveTimer);
    while (savePromise) await savePromise;
    if (messageId && draftAccount != null) api.deleteDraft(draftAccount, messageId).catch((e) => toast(String(e)));
    closeComposer(init.key);
  }

  function onDragOverSheet(e: DragEvent) {
    const types = e.dataTransfer?.types ?? [];
    if (types.includes("application/x-mailvern-msg-id") || types.includes("application/x-mailvern-file")) {
      e.preventDefault();
      if (e.dataTransfer) e.dataTransfer.dropEffect = "copy";
      dropping = true;
    }
  }

  function onDragLeaveSheet(e: DragEvent) {
    if (!e.currentTarget || !(e.currentTarget as HTMLElement).contains(e.relatedTarget as Node | null)) {
      dropping = false;
    }
  }

  async function onDropSheet(e: DragEvent) {
    dropping = false;
    const msgId = e.dataTransfer?.getData("application/x-mailvern-msg-id");
    if (msgId) {
      e.preventDefault();
      try {
        const f = await api.stageEml(Number(msgId));
        addFiles([f]);
        toast(`Attached ${f.name}`);
      } catch (err) {
        toast(String(err));
      }
      return;
    }
    const filePath = e.dataTransfer?.getData("application/x-mailvern-file");
    if (filePath) {
      e.preventDefault();
      try {
        addFiles(await api.fileInfo([filePath]));
      } catch (err) {
        toast(String(err));
      }
    }
  }

  /** Queues the message: now (with a moment to undo) or at `at`. */
  async function send(at: Date | null = null) {
    if (sending) return;
    toField?.flush();
    ccField?.flush();
    bccField?.flush();
    if (!to.length && !cc.length && !bcc.length) {
      toast("Add a recipient");
      return;
    }
    const invalid = [
      ...(toField?.invalidAddresses() ?? []),
      ...(ccField?.invalidAddresses() ?? []),
      ...(bccField?.invalidAddresses() ?? []),
    ];
    if (invalid.length) {
      toast(`Check the email address: ${invalid[0]}`);
      return;
    }
    sending = true;
    try {
      if (!subject.trim() && !(await ask("Send without a subject?", { title: "Mailvern", okLabel: "Send", cancelLabel: "Cancel" }))) return;
      if (attachmentsLoading) await attachmentsLoading;
      if (
        !files.length &&
        mentionsAttachment(editorEl?.innerHTML ?? "") &&
        !(await ask("Your message mentions an attachment, but no files are attached. Send anyway?", {
          title: "Missing attachment?",
          kind: "warning",
          okLabel: "Send anyway",
          cancelLabel: "Go back",
        }))
      ) {
        return;
      }
      const total = files.reduce((n, f) => n + f.size, 0);
      if (total > 24 * 1024 * 1024) {
        toast("Attachments are over 24 MB");
        return;
      }
      clearTimeout(saveTimer);
      // Let a save in progress finish, so it can't put the draft back after sending removes it.
      while (savePromise) await savePromise;
      finished = true;
      const out = outgoing();
      if (draftAccount != null && draftAccount !== accountId && messageId) api.deleteDraft(draftAccount, messageId).catch(() => {});
      const id = await api.send(out, at ? toUnix(at) : null);
      noteSentRecipients([...out.to, ...out.cc, ...out.bcc]);
      closeComposer(init.key);
      loadOutbox();
      if (at) toast(`Scheduled for ${whenLabel(at)}`);
      else if (ui.undoSend > 0) toast("Sending…", { undo: () => editQueued(id), ms: ui.undoSend * 1000 });
      else toast("Sending…");
    } catch (e) {
      finished = false;
      toast(String(e));
    } finally {
      sending = false;
    }
  }

  /** What rephrasing knows about this message: who it's between and what it replies to. */
  function rephraseContext(): RephraseContext {
    return {
      subject,
      from: account?.name ? `${account.name} <${account.email}>` : (account?.email ?? ""),
      to,
      cc,
      source: src,
    };
  }

  const defaultTitle = ({ new: "New message", reply: "Reply", replyAll: "Reply all", forward: "Forward", draft: "Draft" } as const)[init.mode];
  const headerTitle = $derived(subject.trim() || defaultTitle);

  function onKey(e: KeyboardEvent) {
    if (laterOpen) return;
    const combo = comboOf(e);
    if (combo && match(combo) === "send") {
      e.preventDefault();
      send();
    } else if (e.key === "Escape" && !e.defaultPrevented) {
      e.preventDefault();
      if (expanded) toggleExpandComposer(init.key);
      else toggleMinimizeComposer(init.key);
    }
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="wrap" class:expanded class:minimized onmousedown={() => focusComposer(init.key)} onkeydown={onKey}>
  <div
    class="sheet"
    class:dropping
    class:active={isActive}
    class:minimized
    class:expanded
    role="dialog"
    tabindex="-1"
    aria-label={headerTitle}
    ondragover={onDragOverSheet}
    ondragleave={onDragLeaveSheet}
    ondrop={onDropSheet}>
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <header onclick={(e) => { if (minimized && !(e.target as HTMLElement).closest("button")) toggleMinimizeComposer(init.key); }}>
      <span class="title" title={headerTitle}>{headerTitle}</span>
      {#if !minimized}
        <span class="saved">{saving ? "Saving…" : savedAt ? "Saved" : ""}</span>
      {/if}
      <button
        type="button"
        class="icon-btn"
        title={minimized ? "Restore draft" : "Minimize"}
        aria-label={minimized ? "Restore draft" : "Minimize"}
        onclick={() => toggleMinimizeComposer(init.key)}>
        <Icon name="minus" size={15} />
      </button>
      {#if !minimized}
        <button
          type="button"
          class="icon-btn"
          title={expanded ? "Dock composer" : "Expand composer"}
          aria-label={expanded ? "Dock composer" : "Expand composer"}
          onclick={() => toggleExpandComposer(init.key)}>
          <Icon name={expanded ? "minimize" : "maximize"} size={14} />
        </button>
        <button type="button" class="icon-btn" title="Delete draft" aria-label="Delete draft" onclick={discard}><Icon name="trash" size={15} /></button>
      {/if}
      <button type="button" class="icon-btn" title="Save & close" aria-label="Save and close" onclick={close}><Icon name="x" size={15} /></button>
    </header>

    <div class="compose-content" class:hidden={minimized}>
    <div class="fields">
      {#if identities.length > 1}
        <div class="from">
          <span class="label">From</span>
          <Select
            plain
            label="From"
            value={selectedIdentity}
            options={identities.map((o) => ({ value: o.value, label: o.label, detail: o.detail }))}
            onchange={pickIdentity} />
        </div>
      {/if}
      <RecipientField bind:this={toField} label="To" bind:value={to} autofocus={focusTo}>
        {#snippet trailing()}
          <span class="field-toggles">
            {#if !showCc}
              <button type="button" class="cc-toggle" onclick={() => (showCc = true)}>Cc Bcc</button>
            {/if}
            {#if !showReplyTo}
              <button type="button" class="cc-toggle" onclick={() => (showReplyTo = true)}>Reply-To</button>
            {/if}
          </span>
        {/snippet}
      </RecipientField>
      {#if showCc}
        <RecipientField bind:this={ccField} label="Cc" bind:value={cc} />
        <RecipientField bind:this={bccField} label="Bcc" bind:value={bcc} />
      {/if}
      {#if showReplyTo}
        <div class="reply-to">
          <span class="label">Reply-To</span>
          <input bind:value={replyTo} oninput={changed} placeholder="replies@example.com" spellcheck="false" />
        </div>
      {/if}
      <div class="subject">
        <input placeholder="Subject" bind:value={subject} oninput={changed} spellcheck="true" />
      </div>
    </div>

    <div class="body">
      <Editor bind:el={editorEl} html={initialHtml} oninput={changed}>
        {#snippet aboveBar()}
          {#if files.length}
            <div class="files">
              {#each files as f (f.path)}
                <span class="file" title={f.name}>
                  <span class="ficon"><Icon name="file" size={14} /></span>
                  <span class="fname">{f.name}</span>
                  <span class="fsize">{fileSize(f.size)}</span>
                  <button type="button" aria-label={`Remove ${f.name}`} onclick={() => { files = files.filter((x) => x.path !== f.path); changed(); }}><Icon name="x" size={12} /></button>
                </span>
              {/each}
            </div>
          {/if}
        {/snippet}
        <Rephrase editor={editorEl} context={rephraseContext} onapplied={changed} />
        <button type="button" class="icon-btn" title="Attach files" aria-label="Attach files" onclick={pickFiles}><Icon name="clip" size={16} /></button>
        <span class="send-group">
          <button type="button" class="btn primary send" disabled={sending} onclick={() => send()} title={hint("Send", "send")}>
            {#if sending}<span class="spinner"></span>{/if}
            {sending ? "Sending" : "Send"}
          </button>
          <button type="button" class="btn primary later" disabled={sending} onclick={() => (laterOpen = true)} title="Send later" aria-label="Send later">
            <Icon name="chevronDown" size={14} stroke={2.25} />
          </button>
        </span>
      </Editor>
    </div>

    {#if dropping}
      <div class="drop-overlay" aria-hidden="true">
        <div class="drop-box">
          <Icon name="clip" size={22} />
          <span>Drop files or emails to attach</span>
        </div>
      </div>
    {/if}
    </div>
  </div>
</div>

{#if laterOpen}
  <WhenPicker title="Send later" action="Schedule" presets={sendPresets()} onpick={(at) => send(at)} onclose={() => (laterOpen = false)} />
{/if}

<style>
  .wrap {
    pointer-events: auto;
    display: flex;
    align-items: flex-end;
  }
  .wrap.expanded {
    position: fixed;
    inset: 0;
    z-index: 55;
    display: grid;
    place-items: center;
    background: rgba(20, 18, 14, 0.18);
    animation: fade 0.12s ease-out;
  }
  .sheet {
    position: relative;
    display: flex;
    flex-direction: column;
    width: min(540px, calc(100vw - 32px));
    height: min(560px, calc(100vh - 56px));
    padding: 0 18px 14px;
    background: var(--panel);
    border: 1px solid var(--sheet-border);
    border-bottom: 0;
    border-radius: 12px 12px 0 0;
    box-shadow: 0 12px 36px rgba(0, 0, 0, 0.2), 0 2px 8px rgba(0, 0, 0, 0.08);
    animation: rise 0.14s cubic-bezier(0.2, 0.8, 0.2, 1);
    outline: 2px solid transparent;
    transition: outline-color 0.12s;
  }
  .sheet.active:not(.expanded):not(.minimized) {
    border-color: color-mix(in srgb, var(--accent) 45%, var(--sheet-border));
  }
  .sheet.expanded {
    width: min(780px, calc(100vw - 48px));
    height: min(740px, calc(100vh - 64px));
    padding: 0 24px 18px;
    border-bottom: 1px solid var(--sheet-border);
    border-radius: 14px;
    box-shadow: var(--shadow);
  }
  .sheet.minimized {
    width: 260px;
    height: 42px;
    padding: 0 10px 0 14px;
    cursor: pointer;
  }
  .sheet.minimized header {
    height: 42px;
    margin-right: 0;
  }
  .sheet.minimized header:hover { opacity: 0.9; }
  .compose-content {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .compose-content.hidden { display: none; }
  .sheet.dropping { outline-color: var(--accent); }
  .drop-overlay {
    position: absolute;
    inset: 0;
    z-index: 30;
    display: grid;
    place-items: center;
    border-radius: 12px;
    background: color-mix(in srgb, var(--panel) 84%, transparent);
    backdrop-filter: blur(2px);
    pointer-events: none;
  }
  .drop-box {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    padding: 24px 36px;
    border: 2px dashed var(--accent);
    border-radius: 12px;
    background: var(--accent-soft);
    color: var(--accent);
    font-size: 14px;
    font-weight: 550;
  }
  header { display: flex; align-items: center; gap: 2px; height: 46px; margin-right: -6px; flex: none; }
  .title { font-weight: 600; font-size: 14px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; flex: 1; min-width: 0; }
  .saved { flex: none; margin: 0 6px; color: var(--muted); font-size: 12px; }
  .fields { flex: none; }
  .from {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 42px;
    border-bottom: 1px solid var(--line);
  }
  .label { width: 52px; flex: none; color: var(--muted); font-size: 13px; }
  .cc-toggle {
    border: 0;
    background: none;
    color: var(--muted);
    font-size: 12.5px;
    padding: 3px 6px;
    border-radius: 6px;
    white-space: nowrap;
  }
  .cc-toggle:hover { color: var(--text); background: var(--hover); }
  .field-toggles { display: inline-flex; align-items: center; gap: 2px; flex: none; }
  .reply-to {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 40px;
    border-bottom: 1px solid var(--line);
  }
  .reply-to input {
    flex: 1;
    min-width: 0;
    border: 0;
    outline: none;
    background: transparent;
    color: var(--text);
    font-size: 13.5px;
  }
  .subject { border-bottom: 1px solid var(--line); }
  .subject input {
    width: 100%;
    height: 46px;
    border: 0;
    outline: none;
    background: transparent;
    font-size: 16px;
    font-weight: 600;
  }
  .body { flex: 1; min-height: 0; display: flex; flex-direction: column; padding-top: 14px; }
  .send-group { display: inline-flex; }
  .send { min-width: 76px; border-top-right-radius: 0; border-bottom-right-radius: 0; }
  .later {
    width: 28px;
    padding: 0;
    display: grid;
    place-items: center;
    border-top-left-radius: 0;
    border-bottom-left-radius: 0;
    border-left: 1px solid rgba(255, 255, 255, 0.25);
  }
  .spinner {
    width: 12px;
    height: 12px;
    border-radius: 50%;
    border: 2px solid rgba(255, 255, 255, 0.4);
    border-top-color: #fff;
    animation: spin 0.8s linear infinite;
  }
  .files { display: flex; flex-wrap: wrap; gap: 6px; padding: 8px 0 10px; border-top: 1px solid var(--line); }
  .file {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 30px;
    padding: 0 4px 0 10px;
    border: 1px solid var(--line-strong);
    border-radius: 8px;
    color: var(--text-2);
    font-size: 12.5px;
    max-width: min(280px, 100%);
    min-width: 0;
  }
  .ficon { display: grid; flex: none; color: var(--muted); }
  .fname { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--text); font-weight: 500; }
  .fsize { flex: none; color: var(--muted); white-space: nowrap; }
  .file button { flex: none; display: grid; place-items: center; width: 22px; height: 22px; border: 0; border-radius: 5px; background: none; color: var(--muted); }
  .file button:hover { background: var(--hover); color: var(--text); }
</style>
