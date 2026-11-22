<script lang="ts">
  import { ask } from "@tauri-apps/plugin-dialog";
  import Icon from "./Icon.svelte";
  import Editor from "./Editor.svelte";
  import { ui, loadAccounts, loadFolders, setView, toast } from "../state.svelte";
  import { api, type Account } from "../api";
  import { accountColors } from "../format";
  import { guessServers } from "../providers";

  function blank(): Account {
    return {
      id: 0,
      name: "",
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

  let selected = $state<number>(ui.accounts[0]?.id ?? 0);
  let draft = $state<Account>(ui.accounts[0] ? { ...ui.accounts[0] } : blank());
  let password = $state("");
  let advanced = $state(!ui.accounts.length);
  let saving = $state(false);
  let error = $state<string | null>(null);
  let sigEl = $state<HTMLDivElement>();
  let sigSource = $state(false);
  let sigHtml = $state("");
  let hostsTouched = false;

  function choose(id: number) {
    selected = id;
    const a = ui.accounts.find((x) => x.id === id);
    draft = a ? { ...a } : blank();
    password = "";
    error = null;
    advanced = id === 0;
    sigSource = false;
    hostsTouched = id !== 0;
  }

  function onEmail() {
    if (hostsTouched || draft.id) return;
    const s = guessServers(draft.email);
    if (s) Object.assign(draft, s);
  }

  function currentSignature(): string {
    if (sigSource) return sigHtml;
    const html = sigEl?.innerHTML ?? draft.signature;
    return html.replace(/^(<br\s*\/?>|\s)+$/i, "");
  }

  function toggleSource() {
    if (sigSource) {
      draft.signature = sigHtml;
      sigSource = false;
    } else {
      sigHtml = sigEl?.innerHTML ?? "";
      sigSource = true;
    }
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
      const saved = await api.saveAccount({ ...draft, email: draft.email.trim(), signature: currentSignature() }, password || null);
      await loadAccounts();
      await loadFolders();
      choose(saved.id);
      toast("Saved");
      if (ui.accounts.length === 1) setView({ kind: "unified" });
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
    setView({ kind: "unified" });
    choose(ui.accounts[0]?.id ?? 0);
  }

  function close() {
    ui.settings = false;
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="scrim" onkeydown={(e) => e.key === "Escape" && close()}>
  <div class="sheet" role="dialog" aria-label="Accounts">
    <aside>
      <div class="side-title">Accounts</div>
      {#each ui.accounts as a (a.id)}
        <button class="acc" class:active={selected === a.id} onclick={() => choose(a.id)}>
          <span class="dot" style:background={a.color || "var(--muted)"}></span>
          <span class="acc-text">
            <span class="acc-name">{a.name || a.email.split("@")[0]}</span>
            <span class="acc-mail">{a.email}</span>
          </span>
          {#if ui.status[a.id]?.state === "error"}<span class="warn"><Icon name="alert" size={13} /></span>{/if}
        </button>
      {/each}
      <button class="acc add" class:active={selected === 0} onclick={() => choose(0)}>
        <Icon name="plus" size={15} /> Add account
      </button>
    </aside>

    <main class="scroll">
      <header>
        <h2>{draft.id ? draft.name || draft.email : "New account"}</h2>
        <button class="icon-btn" title="Close  Esc" onclick={close}><Icon name="x" /></button>
      </header>

      {#if draft.id && ui.status[draft.id]?.state === "error"}
        <div class="notice">{ui.status[draft.id].message}</div>
      {/if}

      <div class="grid">
        <label>
          <span>Name</span>
          <input class="field" bind:value={draft.name} placeholder="Shown to recipients" />
        </label>
        <label>
          <span>Email</span>
          <input class="field" bind:value={draft.email} oninput={onEmail} placeholder="you@company.com" spellcheck="false" />
        </label>
        <label class="wide">
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
          <button class="icon-btn" class:on={sigSource} title="Edit HTML" onclick={toggleSource}><Icon name="code" size={15} /></button>
        </div>
        <div class="sig">
          {#if sigSource}
            <textarea bind:value={sigHtml} spellcheck="false"></textarea>
          {:else}
            {#key `${selected}-${draft.signature.length}`}
              <Editor bind:el={sigEl} html={draft.signature} minHeight={110} placeholder="Paste or write your signature" />
            {/key}
          {/if}
        </div>
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
        <button class="btn primary" disabled={saving} onclick={save}>
          {saving ? "Connecting…" : draft.id ? "Save" : "Add account"}
        </button>
      </footer>
    </main>
  </div>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 60;
    display: grid;
    place-items: center;
    background: rgba(20, 18, 14, 0.18);
    animation: fade 0.12s ease-out;
  }
  .sheet {
    display: flex;
    width: min(860px, calc(100vw - 48px));
    height: min(680px, calc(100vh - 64px));
    background: var(--panel);
    border-radius: 14px;
    box-shadow: var(--shadow);
    overflow: hidden;
    animation: rise 0.16s cubic-bezier(0.2, 0.8, 0.2, 1);
  }
  aside {
    width: 240px;
    flex: none;
    padding: 18px 10px;
    background: var(--side);
    border-right: 1px solid var(--line);
    overflow-y: auto;
  }
  .side-title { font-size: 12px; font-weight: 600; color: var(--muted); padding: 0 10px 10px; }
  .acc {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 8px 10px;
    border: 0;
    border-radius: 8px;
    background: transparent;
    text-align: left;
  }
  .acc:hover { background: var(--hover); }
  .acc.active { background: var(--active); }
  .dot { width: 9px; height: 9px; border-radius: 50%; flex: none; }
  .acc-text { display: flex; flex-direction: column; min-width: 0; flex: 1; }
  .acc-name { font-weight: 500; font-size: 13.5px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .acc-mail { font-size: 12px; color: var(--muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .acc.add { color: var(--accent); font-weight: 500; font-size: 13.5px; margin-top: 6px; }
  .warn { color: var(--danger); display: grid; }
  main { flex: 1; display: flex; flex-direction: column; padding: 0 28px 22px; min-width: 0; }
  header { display: flex; align-items: center; justify-content: space-between; height: 64px; flex: none; margin-right: -10px; }
  h2 { margin: 0; font-size: 18px; font-weight: 650; letter-spacing: -0.01em; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .notice {
    margin-bottom: 16px;
    padding: 10px 12px;
    border-radius: 8px;
    background: rgba(214, 64, 46, 0.08);
    color: var(--danger);
    font-size: 13px;
    -webkit-user-select: text;
    user-select: text;
  }
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
  .sig {
    display: flex;
    flex-direction: column;
    min-height: 170px;
    padding: 10px 14px;
    border: 1px solid var(--line-strong);
    border-radius: 10px;
  }
  textarea {
    flex: 1;
    min-height: 150px;
    border: 0;
    outline: none;
    background: transparent;
    resize: vertical;
    font: 12.5px/1.5 ui-monospace, "SF Mono", Menlo, monospace;
  }
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
  .error {
    margin-top: 18px;
    padding: 10px 12px;
    border-radius: 8px;
    background: rgba(214, 64, 46, 0.08);
    color: var(--danger);
    font-size: 13px;
    -webkit-user-select: text;
    user-select: text;
  }
  footer { display: flex; align-items: center; gap: 8px; margin-top: auto; padding-top: 24px; }
  .grow { flex: 1; }
</style>
