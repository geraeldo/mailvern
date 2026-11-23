<script lang="ts">
  import { ask } from "@tauri-apps/plugin-dialog";
  import { untrack } from "svelte";
  import Icon from "./Icon.svelte";
  import Editor from "./Editor.svelte";
  import MailFrame from "./MailFrame.svelte";
  import Modal from "./Modal.svelte";
  import { ui, loadAccounts, loadFolders, setView, toast, accountLabel } from "../state.svelte";
  import { api, type Account } from "../api";
  import { accountColors } from "../format";
  import { guessServers } from "../providers";

  function blank(): Account {
    return {
      id: 0,
      name: "",
      label: "",
      email: "",
      imapHost: "",
      imapPort: 993,
      smtpHost: "",
      smtpPort: 465,
      color: accountColors[ui.accounts.length % accountColors.length],
      signature: "",
      position: 0,
    };
  }

  const target = untrack(() => ui.accountSettings);
  const firstId = typeof target === "number" ? target : 0;

  let selected = $state<number>(firstId);
  let draft = $state<Account>(fromId(firstId));
  let password = $state("");
  let advanced = $state(firstId === 0);
  let saving = $state(false);
  let error = $state<string | null>(null);
  let sigEl = $state<HTMLDivElement>();
  let sigMode = $state<"visual" | "html">("visual");
  let sigHtml = $state("");
  let sigKey = $state(0);
  let hostsTouched = firstId !== 0;

  function fromId(id: number): Account {
    const a = ui.accounts.find((x) => x.id === id);
    return a ? { ...a } : blank();
  }

  function choose(id: number) {
    selected = id;
    draft = fromId(id);
    password = "";
    error = null;
    advanced = id === 0;
    sigMode = "visual";
    sigKey++;
    hostsTouched = id !== 0;
  }

  function onEmail() {
    if (hostsTouched || draft.id) return;
    const s = guessServers(draft.email);
    if (s) Object.assign(draft, s);
  }

  function currentSignature(): string {
    const html = sigMode === "html" ? sigHtml : (sigEl?.innerHTML ?? draft.signature);
    return html.trim().replace(/^(<br\s*\/?>|\s|<div><br\s*\/?><\/div>)+$/i, "");
  }

  function setSigMode(mode: "visual" | "html") {
    if (mode === sigMode) return;
    if (mode === "html") {
      sigHtml = currentSignature();
    } else {
      draft.signature = sigHtml;
      sigKey++;
    }
    sigMode = mode;
  }

  async function save() {
    error = null;
    if (!draft.email.includes("@")) {
      error = "Enter an email address";
      return;
    }
    if (!draft.imapHost.trim() || !draft.smtpHost.trim()) {
      advanced = true;
      error = "Enter the incoming and outgoing servers";
      return;
    }
    if (!draft.id && !password) {
      error = "Enter the password";
      return;
    }
    saving = true;
    try {
      const saved = await api.saveAccount(
        { ...draft, email: draft.email.trim(), label: draft.label.trim(), name: draft.name.trim(), signature: currentSignature() },
        password || null,
      );
      const keepMode = sigMode;
      const keepHtml = sigHtml;
      await loadAccounts();
      await loadFolders();
      choose(saved.id);
      if (keepMode === "html") {
        sigMode = "html";
        sigHtml = keepHtml;
      }
      toast("Saved");
      if (ui.accounts.length === 1) setView({ kind: "unified", role: "inbox" });
    } catch (e) {
      error = String(e);
    } finally {
      saving = false;
    }
  }

  async function remove() {
    if (!draft.id) return;
    const ok = await ask(`Remove ${draft.email} from Mailvern? Mail on the server is not affected.`, {
      title: "Remove account",
      kind: "warning",
      okLabel: "Remove",
      cancelLabel: "Cancel",
    });
    if (!ok) return;
    await api.deleteAccount(draft.id);
    await loadAccounts();
    await loadFolders();
    setView({ kind: "unified", role: "inbox" });
    choose(ui.accounts[0]?.id ?? 0);
  }

  function close() {
    ui.accountSettings = null;
  }

  function onKey(e: KeyboardEvent) {
    if (e.metaKey && e.key === "s") {
      e.preventDefault();
      save();
    }
  }
</script>

<Modal label="Accounts" onclose={close} onkeydown={onKey}>
  <aside>
    <div class="side-title">Accounts</div>
    {#each ui.accounts as a (a.id)}
      <button class="nav acc" class:active={selected === a.id} onclick={() => choose(a.id)}>
        <span class="dot" style:background={a.color || "var(--muted)"}></span>
        <span class="acc-text">
          <span class="acc-name">{accountLabel(a)}</span>
          <span class="acc-mail">{a.email}</span>
        </span>
        {#if ui.status[a.id]?.state === "error"}<span class="warn"><Icon name="alert" size={13} /></span>{/if}
      </button>
    {/each}
    <button class="nav add" class:active={selected === 0} onclick={() => choose(0)}>
      <Icon name="plus" size={15} /> Add account
    </button>
  </aside>

  <main class="scroll">
    <header>
      <h2>
        {#if draft.id}{accountLabel(draft)}{:else}New account{/if}
      </h2>
      <button class="icon-btn" title="Close  Esc" onclick={close}><Icon name="x" /></button>
    </header>

    {#if draft.id && ui.status[draft.id]?.state === "error"}
      <div class="notice">{ui.status[draft.id].message}</div>
    {/if}

    <div class="grid">
      <label>
        <span>Label</span>
        <input class="field" bind:value={draft.label} placeholder="Work, Personal…" />
      </label>
      <label>
        <span>Sender name</span>
        <input class="field" bind:value={draft.name} placeholder="Your name" />
      </label>
      <label>
        <span>Email</span>
        <input class="field" bind:value={draft.email} oninput={onEmail} placeholder="you@company.com" spellcheck="false" />
      </label>
      <label>
        <span>Password</span>
        <input
          class="field"
          type="password"
          bind:value={password}
          placeholder={draft.id ? "Saved in Keychain" : "Password or app password"}
          autocomplete="off" />
      </label>
      <div class="wide">
        <span class="lbl">Color</span>
        <div class="swatches">
          {#each accountColors as c (c)}
            <button
              class="swatch"
              class:on={draft.color === c}
              style:background={c}
              aria-label="Color"
              onclick={() => (draft.color = c)}></button>
          {/each}
        </div>
      </div>
    </div>

    <section>
      <div class="sec-head">
        <h3>Signature</h3>
        <div class="seg" role="tablist">
          <button role="tab" class:on={sigMode === "visual"} onclick={() => setSigMode("visual")}>Visual</button>
          <button role="tab" class:on={sigMode === "html"} onclick={() => setSigMode("html")}>HTML</button>
        </div>
      </div>
      {#if sigMode === "visual"}
        <div class="sig">
          {#key `${selected}-${sigKey}`}
            <Editor bind:el={sigEl} html={draft.signature} minHeight={110} placeholder="Write or paste your signature" />
          {/key}
        </div>
      {:else}
        <div class="sig html">
          <textarea bind:value={sigHtml} spellcheck="false" placeholder="<table>…</table>"></textarea>
        </div>
        <div class="preview-label">Preview</div>
        <div class="preview">
          <MailFrame html={sigHtml || "<span style='color:#6b6760'>Nothing yet</span>"} allowRemote={true} />
        </div>
      {/if}
    </section>

    <section>
      <button class="disclose" onclick={() => (advanced = !advanced)}>
        <span class="chev" class:open={advanced}><Icon name="chevron" size={12} stroke={2} /></span>
        Servers
      </button>
      {#if advanced}
        <div class="grid servers">
          <label>
            <span>Incoming (IMAP)</span>
            <input class="field" bind:value={draft.imapHost} oninput={() => (hostsTouched = true)} spellcheck="false" />
          </label>
          <label class="port">
            <span>Port</span>
            <input class="field" type="number" bind:value={draft.imapPort} />
          </label>
          <label>
            <span>Outgoing (SMTP)</span>
            <input class="field" bind:value={draft.smtpHost} oninput={() => (hostsTouched = true)} spellcheck="false" />
          </label>
          <label class="port">
            <span>Port</span>
            <input class="field" type="number" bind:value={draft.smtpPort} />
          </label>
        </div>
        <p class="hint">SSL on 993 and 465. STARTTLS on 587.</p>
      {/if}
    </section>

    {#if error}<div class="error">{error}</div>{/if}

    <footer>
      {#if draft.id}
        <button class="btn ghost danger" onclick={remove}>Remove</button>
      {/if}
      <span class="grow"></span>
      <button class="btn primary" disabled={saving} onclick={save} title="Save  ⌘S">
        {saving ? "Connecting…" : draft.id ? "Save" : "Add account"}
      </button>
    </footer>
  </main>
</Modal>

<style>
  aside {
    width: 240px;
    flex: none;
    display: flex;
    flex-direction: column;
    padding: 18px 10px 12px;
    background: var(--side);
    border-right: 1px solid var(--line);
    overflow-y: auto;
  }
  .side-title { font-size: 12px; font-weight: 600; color: var(--muted); padding: 0 10px 10px; }
  .nav {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 8px 10px;
    border: 0;
    border-radius: 8px;
    background: transparent;
    text-align: left;
    font-size: 13.5px;
    color: var(--text-2);
  }
  .nav:hover { background: var(--hover); }
  .nav.active { background: var(--active); color: var(--text); }
  .nav :global(svg) { color: var(--muted); flex: none; }
  .dot { width: 9px; height: 9px; border-radius: 50%; flex: none; }
  .acc-text { display: flex; flex-direction: column; min-width: 0; flex: 1; }
  .acc-name { font-weight: 500; color: var(--text); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .acc-mail { font-size: 12px; color: var(--muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .nav.add { color: var(--accent); font-weight: 500; margin-top: 6px; }
  .nav.add :global(svg) { color: var(--accent); }
  .grow { flex: 1; }
  .warn { color: var(--danger); display: grid; }
  main { flex: 1; display: flex; flex-direction: column; padding: 0 28px 22px; min-width: 0; }
  header { display: flex; align-items: center; justify-content: space-between; height: 64px; flex: none; margin-right: -10px; }
  h2 { margin: 0; font-size: 18px; font-weight: 650; letter-spacing: -0.01em; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .notice, .error {
    margin-bottom: 16px;
    padding: 10px 12px;
    border-radius: 8px;
    background: rgba(214, 64, 46, 0.08);
    color: var(--danger);
    font-size: 13px;
    -webkit-user-select: text;
    user-select: text;
  }
  .error { margin: 18px 0 0; }
  .grid { display: grid; grid-template-columns: 1fr 1fr; gap: 16px; }
  .wide { grid-column: span 2; }
  label { display: flex; flex-direction: column; gap: 6px; }
  label span, .lbl { font-size: 12.5px; font-weight: 500; color: var(--text-2); }
  .swatches { display: flex; gap: 8px; margin-top: 8px; }
  .swatch { width: 22px; height: 22px; border-radius: 50%; border: 0; padding: 0; box-shadow: inset 0 0 0 1px rgba(0, 0, 0, 0.08); }
  .swatch.on { box-shadow: 0 0 0 2px var(--panel), 0 0 0 4px var(--text); }
  section { margin-top: 26px; }
  .sec-head { display: flex; align-items: center; justify-content: space-between; margin-bottom: 8px; }
  h3 { margin: 0; font-size: 13.5px; font-weight: 600; }
  .seg { display: inline-flex; padding: 2px; border-radius: 8px; background: var(--hover); }
  .seg button {
    height: 24px;
    padding: 0 10px;
    border: 0;
    border-radius: 6px;
    background: transparent;
    font-size: 12px;
    font-weight: 500;
    color: var(--muted);
  }
  .seg button.on { background: var(--panel); color: var(--text); box-shadow: 0 1px 2px rgba(0, 0, 0, 0.08); }
  .sig {
    display: flex;
    flex-direction: column;
    min-height: 180px;
    padding: 10px 14px;
    border: 1px solid var(--line-strong);
    border-radius: 10px;
  }
  .sig.html { padding: 0; }
  textarea {
    flex: 1;
    min-height: 180px;
    padding: 12px 14px;
    border: 0;
    border-radius: 10px;
    outline: none;
    background: transparent;
    resize: vertical;
    font: 12.5px/1.55 ui-monospace, "SF Mono", Menlo, monospace;
  }
  .preview-label { font-size: 12px; font-weight: 600; color: var(--muted); margin: 14px 0 6px; }
  .preview { padding: 12px 14px; border-radius: 10px; background: #fff; border: 1px solid var(--line); }
  .disclose {
    display: flex;
    align-items: center;
    gap: 6px;
    border: 0;
    background: none;
    padding: 0;
    font-size: 13.5px;
    font-weight: 600;
  }
  .chev { display: grid; color: var(--muted); transition: transform 0.15s; }
  .chev.open { transform: rotate(90deg); }
  .servers { grid-template-columns: 1fr 96px; margin-top: 14px; }
  .hint { margin: 10px 0 0; font-size: 12px; color: var(--muted); }
  footer { display: flex; align-items: center; gap: 8px; margin-top: auto; padding-top: 24px; }
</style>
