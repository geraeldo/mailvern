<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { open as openDialog, ask } from "@tauri-apps/plugin-dialog";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import Icon from "./Icon.svelte";
  import Editor from "./Editor.svelte";
  import RecipientField from "./RecipientField.svelte";
  import { ui, toast, folderById, type ComposeInit } from "../state.svelte";
  import { api, type FileInfo } from "../api";
  import { formatAddr, fileSize } from "../format";
  import { htmlToText, prefixed, quoteForward, quoteReply, emailWrap } from "../html";
  import { comboOf, match, hint } from "../shortcuts.svelte";

  let props: { init: ComposeInit } = $props();
  // The composer is keyed per open, so its setup is read once.
  const init = untrack(() => props.init);
  const src = init.source;
  const myEmails = new Set(ui.accounts.map((a) => a.email.toLowerCase()));
  const notMe = (e: string) => !myEmails.has(e.toLowerCase());

  function defaultAccount(): number {
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
  let initSubject = "";
  if (src) {
    const sender = src.replyTo.length ? src.replyTo : [{ name: src.fromName || null, email: src.fromAddr }];
    if (init.mode === "reply" || init.mode === "replyAll") {
      initTo = sender.map(formatAddr);
      initSubject = prefixed("Re", src.subject);
      if (init.mode === "replyAll") {
        const seen = new Set(sender.map((a) => a.email.toLowerCase()));
        initTo = [...initTo, ...src.to.filter((a) => notMe(a.email) && !seen.has(a.email.toLowerCase())).map(formatAddr)];
        initCc = src.cc.filter((a) => notMe(a.email)).map(formatAddr);
      }
    } else if (init.mode === "forward") {
      initSubject = prefixed("Fwd", src.subject);
    }
  }
  if (init.to) initTo = init.to;
  const initAccount = defaultAccount();

  let accountId = $state(initAccount);
  let to = $state<string[]>(initTo);
  let cc = $state<string[]>(initCc);
  let bcc = $state<string[]>([]);
  let showCc = $state(initCc.length > 0);
  let subject = $state(initSubject);
  let files = $state<FileInfo[]>([]);
  let sending = $state(false);
  let dirty = $state(false);
  let editorEl = $state<HTMLDivElement>();
  let dropping = $state(false);
  let toField: RecipientField;
  let ccField = $state<RecipientField>();
  let bccField = $state<RecipientField>();

  const account = $derived(ui.accounts.find((a) => a.id === accountId)!);

  function sigBlock(sig: string) {
    return `<div data-signature="1">${sig ? `<br>${sig}` : ""}</div>`;
  }

  const signature = ui.accounts.find((a) => a.id === initAccount)?.signature ?? "";
  const quote = src ? (init.mode === "forward" ? quoteForward(src) : quoteReply(src)) : "";
  const initialHtml = `<div><br></div>${sigBlock(signature)}${quote ? `<br>${quote}` : ""}`;

  onMount(() => {
    if (src && init.mode === "forward" && src.attachments.length) {
      api.attachmentFiles(src.id).then((f) => (files = [...f, ...files])).catch((e) => toast(String(e)));
    }
    // Put the caret at the top of the body for replies; focus "To" otherwise.
    if (init.mode === "reply" || init.mode === "replyAll") {
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
      un.then((f) => f());
    };
  });

  // Swap the signature when the sending account changes.
  let lastAccount = initAccount;
  $effect(() => {
    if (accountId === lastAccount || !editorEl) return;
    lastAccount = accountId;
    const block = editorEl.querySelector<HTMLElement>("[data-signature]");
    const sig = account?.signature ?? "";
    if (block) block.innerHTML = sig ? `<br>${sig}` : "";
  });

  function addFiles(list: FileInfo[]) {
    const have = new Set(files.map((f) => f.path));
    files = [...files, ...list.filter((f) => !have.has(f.path))];
    dirty = true;
  }

  async function pickFiles() {
    const picked = await openDialog({ multiple: true, directory: false });
    if (!picked) return;
    addFiles(await api.fileInfo(Array.isArray(picked) ? picked : [picked]));
  }

  async function close() {
    if (sending) return;
    if (dirty && !(await ask("Discard this message?", { title: "Mailvern", kind: "warning", okLabel: "Discard", cancelLabel: "Keep editing" }))) return;
    ui.compose = null;
  }

  async function send() {
    if (sending) return;
    toField?.flush();
    ccField?.flush();
    bccField?.flush();
    if (!to.length && !cc.length && !bcc.length) {
      toast("Add a recipient");
      return;
    }
    if (!subject.trim() && !(await ask("Send without a subject?", { title: "Mailvern", okLabel: "Send", cancelLabel: "Cancel" }))) return;
    const html = emailWrap(editorEl?.innerHTML ?? "");
    const total = files.reduce((n, f) => n + f.size, 0);
    if (total > 24 * 1024 * 1024) {
      toast("Attachments are over 24 MB");
      return;
    }
    sending = true;
    try {
      await api.send({
        accountId,
        to,
        cc,
        bcc,
        subject: subject.trim(),
        html,
        text: htmlToText(html),
        attachments: files.map((f) => f.path),
        inReplyTo: init.mode === "forward" ? null : (src?.messageId ?? null),
        references: src?.messageId ? [src.references, src.messageId].filter(Boolean).join(" ") : null,
      });
      ui.compose = null;
      toast("Sent");
    } catch (e) {
      toast(String(e));
    } finally {
      sending = false;
    }
  }

  function onKey(e: KeyboardEvent) {
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
      <span class="title">{({ new: "New message", reply: "Reply", replyAll: "Reply all", forward: "Forward" } as const)[init.mode]}</span>
      <button class="icon-btn" title="Close  Esc" onclick={close}><Icon name="x" /></button>
    </header>

    <div class="fields">
      {#if ui.accounts.length > 1}
        <div class="from">
          <span class="label">From</span>
          <select bind:value={accountId}>
            {#each ui.accounts as a (a.id)}
              <option value={a.id}>{a.name ? `${a.name} <${a.email}>` : a.email}</option>
            {/each}
          </select>
        </div>
      {/if}
      <div class="to-wrap">
        <RecipientField bind:this={toField} label="To" bind:value={to} autofocus={init.mode === "new" || init.mode === "forward"} />
        {#if !showCc}
          <button class="cc-toggle" onclick={() => (showCc = true)}>Cc Bcc</button>
        {/if}
      </div>
      {#if showCc}
        <RecipientField bind:this={ccField} label="Cc" bind:value={cc} />
        <RecipientField bind:this={bccField} label="Bcc" bind:value={bcc} />
      {/if}
      <div class="subject">
        <input placeholder="Subject" bind:value={subject} oninput={() => (dirty = true)} spellcheck="true" />
      </div>
    </div>

    <div class="body">
      <Editor bind:el={editorEl} html={initialHtml} oninput={() => (dirty = true)}>
        <button class="icon-btn" title="Attach files" onclick={pickFiles}><Icon name="clip" size={16} /></button>
        <button class="btn primary send" disabled={sending} onclick={send} title={hint("Send", "send")}>
          {#if sending}<span class="spinner"></span>{/if}
          {sending ? "Sending" : "Send"}
        </button>
      </Editor>
    </div>

    {#if files.length}
      <div class="files">
        {#each files as f (f.path)}
          <span class="file">
            <Icon name="file" size={14} />
            <span class="fname">{f.name}</span>
            <span class="fsize">{fileSize(f.size)}</span>
            <button aria-label="Remove" onclick={() => (files = files.filter((x) => x.path !== f.path))}><Icon name="x" size={12} /></button>
          </span>
        {/each}
      </div>
    {/if}
  </div>
</div>

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
    display: flex;
    flex-direction: column;
    width: min(760px, calc(100vw - 48px));
    height: min(720px, calc(100vh - 64px));
    padding: 0 24px 18px;
    background: var(--panel);
    border-radius: 14px;
    box-shadow: var(--shadow);
    animation: rise 0.16s cubic-bezier(0.2, 0.8, 0.2, 1);
    outline: 2px solid transparent;
    transition: outline-color 0.12s;
  }
  .sheet.dropping { outline-color: var(--accent); }
  header { display: flex; align-items: center; justify-content: space-between; height: 52px; margin-right: -10px; }
  .title { font-weight: 600; font-size: 15px; }
  .fields { flex: none; }
  .from {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 42px;
    border-bottom: 1px solid var(--line);
  }
  .label { width: 52px; flex: none; color: var(--muted); font-size: 13px; }
  select {
    flex: 1;
    border: 0;
    background: transparent;
    outline: none;
    font-size: 14px;
    margin-left: -4px;
  }
  .to-wrap { position: relative; }
  .cc-toggle {
    position: absolute;
    right: 0;
    top: 10px;
    border: 0;
    background: none;
    color: var(--muted);
    font-size: 12.5px;
    padding: 2px 4px;
    border-radius: 5px;
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
  .send { min-width: 84px; }
  .spinner {
    width: 12px;
    height: 12px;
    border-radius: 50%;
    border: 2px solid rgba(255, 255, 255, 0.4);
    border-top-color: #fff;
    animation: spin 0.8s linear infinite;
  }
  .files { display: flex; flex-wrap: wrap; gap: 6px; padding-top: 12px; }
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
    max-width: 280px;
  }
  .fname { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--text); font-weight: 500; }
  .fsize { color: var(--muted); white-space: nowrap; }
  .file button { display: grid; place-items: center; width: 22px; height: 22px; border: 0; border-radius: 5px; background: none; color: var(--muted); }
  .file button:hover { background: var(--hover); color: var(--text); }
</style>
