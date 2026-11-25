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
  import { ui, toast, folderById, editQueued, loadOutbox, noteSentRecipients, type ComposeInit } from "../state.svelte";
  import { api, type FileInfo, type Outgoing } from "../api";
  import { sendPresets, toUnix, whenLabel } from "../when";
  import { formatAddr, fileSize } from "../format";
  import { htmlToText, prefixed, quoteForward, quoteReply, emailWrap, sanitize } from "../html";
  import { comboOf, match, hint } from "../shortcuts.svelte";

  let props: { init: ComposeInit } = $props();
  // The composer is keyed per open, so its setup is read once.
  const init = untrack(() => props.init);
  const src = init.source;
  const restore = init.restore;
  const myEmails = new Set(ui.accounts.map((a) => a.email.toLowerCase()));
  const notMe = (e: string) => !myEmails.has(e.toLowerCase());

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
  if (restore) {
    initTo = restore.message.to;
    initCc = restore.message.cc;
    initBcc = restore.message.bcc;
    initSubject = restore.message.subject;
  }
  const initAccount = defaultAccount();
  /** Where typing starts: "To" for a fresh message, else the body. */
  const focusTo = init.mode === "new" || init.mode === "forward" || (init.mode === "draft" && !initTo.length);
  const inReplyTo = restore ? restore.message.inReplyTo : init.mode === "forward" ? null : (src?.messageId ?? null);
  const references = restore
    ? restore.message.references
    : src?.messageId
      ? [src.references, src.messageId].filter(Boolean).join(" ")
      : null;

  let accountId = $state(initAccount);
  let to = $state<string[]>(initTo);
  let cc = $state<string[]>(initCc);
  let bcc = $state<string[]>(initBcc);
  let showCc = $state(initCc.length > 0 || initBcc.length > 0);
  let subject = $state(initSubject);
  let files = $state<FileInfo[]>(restore?.attachments ?? []);
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

  function sigBlock(sig: string) {
    const clean = sig ? sanitize(sig) : "";
    return `<div data-signature="1">${clean ? `<br>${clean}` : ""}</div>`;
  }

  const signature = ui.accounts.find((a) => a.id === initAccount)?.signature ?? "";
  const quote = src ? (init.mode === "forward" ? quoteForward(src) : quoteReply(src)) : "";
  const initialHtml = restore ? restore.message.html : `<div><br></div>${sigBlock(signature)}${quote ? `<br>${quote}` : ""}`;

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
    JSON.stringify([to, cc, bcc, accountId]);
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
    ui.compose = null;
  }

  async function discard() {
    if (sending) return;
    if ((dirty || draftAccount != null) && !(await ask("Delete this draft?", { title: "Mailvern", kind: "warning", okLabel: "Delete", cancelLabel: "Keep editing" }))) return;
    finished = true;
    clearTimeout(saveTimer);
    while (savePromise) await savePromise;
    if (messageId && draftAccount != null) api.deleteDraft(draftAccount, messageId).catch((e) => toast(String(e)));
    ui.compose = null;
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
      ui.compose = null;
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

  function onKey(e: KeyboardEvent) {
    if (laterOpen) return;
    const combo = comboOf(e);
    if (combo && match(combo) === "send") {
      e.preventDefault();
      send();
    } else if (e.key === "Escape" && !e.defaultPrevented) {
      e.preventDefault();
      close();
    }
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="scrim" onkeydown={onKey}>
  <div class="sheet" class:dropping role="dialog" aria-label="New message">
    <header>
      <span class="title">{({ new: "New message", reply: "Reply", replyAll: "Reply all", forward: "Forward", draft: "Draft" } as const)[init.mode]}</span>
      <span class="saved">{saving ? "Saving…" : savedAt ? "Saved to Drafts" : ""}</span>
      <button class="icon-btn" title="Delete draft" onclick={discard}><Icon name="trash" size={16} /></button>
      <button class="icon-btn" title="Close  Esc" onclick={close}><Icon name="x" /></button>
    </header>

    <div class="fields">
      {#if ui.accounts.length > 1}
        <div class="from">
          <span class="label">From</span>
          <Select
            plain
            label="From"
            value={accountId}
            options={ui.accounts.map((a) => (a.name ? { value: a.id, label: a.name, detail: a.email } : { value: a.id, label: a.email }))}
            onchange={(id) => (accountId = id)} />
        </div>
      {/if}
      <RecipientField bind:this={toField} label="To" bind:value={to} autofocus={focusTo}>
        {#snippet trailing()}
          {#if !showCc}
            <button type="button" class="cc-toggle" onclick={() => (showCc = true)}>Cc Bcc</button>
          {/if}
        {/snippet}
      </RecipientField>
      {#if showCc}
        <RecipientField bind:this={ccField} label="Cc" bind:value={cc} />
        <RecipientField bind:this={bccField} label="Bcc" bind:value={bcc} />
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
          <span>Drop files to attach</span>
        </div>
      </div>
    {/if}
  </div>
</div>

{#if laterOpen}
  <WhenPicker title="Send later" action="Schedule" presets={sendPresets()} onpick={(at) => send(at)} onclose={() => (laterOpen = false)} />
{/if}

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 50;
    display: grid;
    place-items: center;
    background: rgba(20, 18, 14, 0.18);
    animation: fade 0.12s ease-out;
  }
  .sheet {
    position: relative;
    display: flex;
    flex-direction: column;
    width: min(760px, calc(100vw - 48px));
    height: min(720px, calc(100vh - 64px));
    padding: 0 24px 18px;
    background: var(--panel);
    border: 1px solid var(--sheet-border);
    border-radius: 14px;
    box-shadow: var(--shadow);
    animation: rise 0.16s cubic-bezier(0.2, 0.8, 0.2, 1);
    outline: 2px solid transparent;
    transition: outline-color 0.12s;
  }
  .sheet.dropping { outline-color: var(--accent); }
  .drop-overlay {
    position: absolute;
    inset: 0;
    z-index: 30;
    display: grid;
    place-items: center;
    border-radius: 14px;
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
  header { display: flex; align-items: center; gap: 2px; height: 52px; margin-right: -10px; }
  .title { font-weight: 600; font-size: 15px; }
  .saved { flex: 1; margin-left: 10px; color: var(--muted); font-size: 12.5px; }
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
