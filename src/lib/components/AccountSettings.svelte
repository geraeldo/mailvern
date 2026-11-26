<script lang="ts">
  import { ask } from "@tauri-apps/plugin-dialog";
  import Icon from "./Icon.svelte";
  import Editor from "./Editor.svelte";
  import MailFrame from "./MailFrame.svelte";
  import { ui, loadAccounts, loadFolders, setView, toast, checkAccountForMail } from "../state.svelte";
  import { api, type Account } from "../api";
  import { accountColors } from "../format";
  import { detectProvider, guessServers } from "../providers";
  import { sanitize } from "../html";

  let {
    target,
    firstRun = false,
    onselect,
    onclose,
  }: {
    target: number | "new";
    firstRun?: boolean;
    onselect?: (id: number | "new") => void;
    onclose?: () => void;
  } = $props();

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
      aliases: [],
      replyTo: "",
      notify: true,
      badgeMode: "",
    };
  }

  function fromTarget(t: number | "new"): Account {
    if (typeof t === "number") {
      const a = ui.accounts.find((x) => x.id === t);
      if (a) {
        return {
          ...a,
          aliases: (a.aliases ?? []).map((x) => ({ ...x })),
          replyTo: a.replyTo ?? "",
          notify: a.notify ?? true,
          badgeMode: a.badgeMode ?? "",
        };
      }
    }
    return blank();
  }

  let draft = $state<Account>(blank());
  let originSnapshot = $state("");
  let password = $state("");
  let advanced = $state(false);
  let customize = $state(false);
  let saving = $state(false);
  let error = $state<string | null>(null);
  let sigEl = $state<HTMLDivElement>();
  let sigMode = $state<"visual" | "html">("visual");
  let sigHtml = $state("");
  let sigKey = $state(0);
  let newAliasName = $state("");
  let newAliasEmail = $state("");
  let aliasError = $state("");
  let hostsTouched = false;
  let loadedTarget: number | "new" | null = null;

  const provider = $derived(detectProvider(draft.email));

  function snapshotOf(a: Account, sig: string): string {
    return JSON.stringify({
      label: a.label,
      name: a.name,
      email: a.email,
      imapHost: a.imapHost,
      imapPort: a.imapPort,
      smtpHost: a.smtpHost,
      smtpPort: a.smtpPort,
      color: a.color,
      signature: sig,
      aliases: a.aliases,
      replyTo: a.replyTo,
      notify: a.notify,
      badgeMode: a.badgeMode,
    });
  }

  function addAlias() {
    const email = newAliasEmail.trim();
    const name = newAliasName.trim();
    if (!email || !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email)) {
      aliasError = "Enter a valid email address";
      return;
    }
    if (
      email.toLowerCase() === draft.email.trim().toLowerCase() ||
      draft.aliases.some((a) => a.email.toLowerCase() === email.toLowerCase())
    ) {
      aliasError = "That address is already on this account";
      return;
    }
    draft.aliases = [...draft.aliases, { name, email }];
    newAliasName = "";
    newAliasEmail = "";
    aliasError = "";
  }

  function removeAlias(email: string) {
    draft.aliases = draft.aliases.filter((a) => a.email !== email);
  }

  function loadTarget(t: number | "new") {
    const next = fromTarget(t);
    draft = next;
    password = "";
    error = null;
    advanced = false;
    customize = next.id !== 0;
    sigMode = "visual";
    sigHtml = next.signature;
    sigKey++;
    hostsTouched = next.id !== 0;
    originSnapshot = snapshotOf(next, normalizeSignature(next.signature));
    loadedTarget = t;
  }

  $effect(() => {
    if (target !== loadedTarget) loadTarget(target);
  });

  function normalizeSignature(html: string): string {
    return sanitize(html).trim().replace(/^(<br\s*\/?>|\s|<div><br\s*\/?><\/div>)+$/i, "");
  }

  function currentSignature(): string {
    const html = sigMode === "html" ? sigHtml : (sigEl?.dataset.ready ? sigEl.innerHTML : draft.signature);
    return normalizeSignature(html);
  }

  export function isDirty(): boolean {
    if (loadedTarget === null) return false;
    if (password.length > 0) return true;
    return snapshotOf(draft, currentSignature()) !== originSnapshot;
  }

  function onEmail() {
    if (hostsTouched || draft.id) return;
    const p = detectProvider(draft.email);
    if (p) {
      Object.assign(draft, p.servers);
      advanced = false;
      return;
    }
    const s = guessServers(draft.email);
    if (s) {
      Object.assign(draft, s);
      // Custom domain: reveal servers so the user can confirm/adjust.
      advanced = true;
    }
  }

  function setSigMode(mode: "visual" | "html") {
    if (mode === sigMode) return;
    if (mode === "html") {
      sigHtml = currentSignature();
    } else {
      draft.signature = sanitize(sigHtml);
      sigKey++;
    }
    sigMode = mode;
  }

  export async function save() {
    if (saving) return;
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
      const wasFirst = ui.accounts.length === 0;
      const saved = await api.saveAccount(
        {
          ...draft,
          email: draft.email.trim(),
          label: draft.label.trim(),
          name: draft.name.trim(),
          replyTo: draft.replyTo.trim(),
          aliases: draft.aliases.map((a) => ({ name: a.name.trim(), email: a.email.trim() })).filter((a) => a.email),
          signature: currentSignature(),
        },
        password || null,
      );
      const keepMode = sigMode;
      const keepHtml = sigHtml;
      await loadAccounts();
      await loadFolders();
      loadTarget(saved.id);
      if (keepMode === "html") {
        sigMode = "html";
        sigHtml = keepHtml;
      }
      onselect?.(saved.id);
      toast(wasFirst ? "Account connected" : "Saved");
      if (wasFirst) {
        setView({ kind: "unified", role: "inbox" });
        onclose?.();
      }
    } catch (e) {
      error = String(e);
    } finally {
      saving = false;
    }
  }

  async function remove() {
    if (saving || !draft.id) return;
    const ok = await ask(`Remove ${draft.email}? Mail on the server stays.`, {
      title: "Remove account",
      kind: "warning",
      okLabel: "Remove",
      cancelLabel: "Cancel",
    });
    if (!ok) return;
    saving = true;
    error = null;
    try {
      await api.deleteAccount(draft.id);
      await loadAccounts();
      await loadFolders();
      setView({ kind: "unified", role: "inbox" });
      const next = ui.accounts[0]?.id ?? "new";
      loadTarget(next);
      onselect?.(next);
    } catch (e) {
      error = String(e);
    } finally {
      saving = false;
    }
  }

  function onKey(e: KeyboardEvent) {
    if (e.metaKey && e.key.toLowerCase() === "s") {
      e.preventDefault();
      save();
    }
  }
</script>

<svelte:window onkeydown={onKey} />

<div class="acc-pane">
  {#if firstRun && !draft.id}
    <p class="welcome">Connect an email account over IMAP and SMTP. Your credentials stay in your Mac's Keychain.</p>
  {/if}

  {#if draft.id && ui.status[draft.id]?.state === "error"}
    <div class="notice">
      <Icon name="alert" size={15} />
      <span class="notice-msg">{ui.status[draft.id].message || "Couldn't connect to this account."}</span>
      <button type="button" class="btn sm" onclick={() => checkAccountForMail(draft.id)}>Retry</button>
    </div>
  {/if}

  <div class="card">
    <div class="grid">
      <label>
        <span>Email</span>
        <input class="field" bind:value={draft.email} oninput={onEmail} placeholder="you@company.com" spellcheck="false" autocomplete="username" />
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
    </div>

    {#if !draft.id && provider}
      <div class="detected">
        <span class="detected-ok"><Icon name="check" size={13} stroke={2.4} /> {provider.name} servers configured</span>
        <button type="button" class="link" onclick={() => (advanced = !advanced)}>{advanced ? "Hide servers" : "Edit"}</button>
      </div>
    {/if}
    {#if !draft.id && provider?.passwordHint}
      <p class="hint">{provider.passwordHint}</p>
    {/if}
  </div>

  {#if draft.id || customize}
    <section>
      <div class="sec-title">Profile</div>
      <div class="card">
        <div class="grid">
          <label>
            <span>Label</span>
            <input class="field" bind:value={draft.label} placeholder="Work, Personal…" />
          </label>
          <label>
            <span>Sender name</span>
            <input class="field" bind:value={draft.name} placeholder="Your name" />
          </label>
          <label class="wide">
            <span>Default Reply-To</span>
            <input class="field" bind:value={draft.replyTo} placeholder="Optional (e.g. replies@company.com)" spellcheck="false" />
          </label>
          <div class="wide">
            <span class="lbl">Color</span>
            <div class="swatches">
              {#each accountColors as c (c)}
                <button
                  type="button"
                  class="swatch"
                  class:on={draft.color === c}
                  style:background={c}
                  aria-label="Account color"
                  onclick={() => (draft.color = c)}></button>
              {/each}
            </div>
          </div>
        </div>
      </div>
    </section>

    <section>
      <div class="sec-title">Send-as aliases</div>
      <div class="card">
        <div class="alias-add">
          <input class="field" bind:value={newAliasName} placeholder="Display name (optional)" />
          <input
            class="field"
            bind:value={newAliasEmail}
            placeholder="alias@domain.com"
            spellcheck="false"
            oninput={() => (aliasError = "")}
            onkeydown={(e) => { if (e.key === "Enter") { e.preventDefault(); addAlias(); } }} />
          <button type="button" class="btn sm" onclick={addAlias}>Add</button>
        </div>
        {#if aliasError}<p class="hint err">{aliasError}</p>{/if}
        {#if draft.aliases.length}
          <div class="alias-list">
            {#each draft.aliases as a (a.email)}
              <div class="alias-row">
                <span class="alias-info">
                  {#if a.name}<strong>{a.name}</strong> {/if}
                  <span class="alias-mail">{a.email}</span>
                </span>
                <button type="button" class="icon-btn sm" title="Remove alias" aria-label={`Remove ${a.email}`} onclick={() => removeAlias(a.email)}>
                  <Icon name="x" size={14} />
                </button>
              </div>
            {/each}
          </div>
        {:else}
          <p class="hint">Add extra addresses configured on your mail provider to choose from in the composer.</p>
        {/if}
      </div>
    </section>

    <section>
      <div class="sec-head">
        <span class="sec-title">Signature</span>
        <div class="seg" role="tablist">
          <button type="button" role="tab" aria-selected={sigMode === "visual"} class:on={sigMode === "visual"} onclick={() => setSigMode("visual")}>Visual</button>
          <button type="button" role="tab" aria-selected={sigMode === "html"} class:on={sigMode === "html"} onclick={() => setSigMode("html")}>HTML</button>
        </div>
      </div>
      {#if sigMode === "visual"}
        <div class="sig">
          {#key `${target}-${sigKey}`}
            <Editor bind:el={sigEl} html={sanitize(draft.signature)} minHeight={110} placeholder="Write or paste your signature" />
          {/key}
        </div>
      {:else}
        <div class="sig html">
          <textarea bind:value={sigHtml} spellcheck="false" placeholder="<table>…</table>"></textarea>
        </div>
        <div class="preview-label">Preview</div>
        <div class="preview">
          <MailFrame html={sigHtml || "<span style='color:#6b6760'>Nothing yet</span>"} allowRemote={false} />
        </div>
      {/if}
    </section>
  {:else}
    <button type="button" class="disclose" onclick={() => (customize = true)}>
      <span class="chev"><Icon name="chevron" size={12} stroke={2} /></span>
      Customize name, color &amp; signature
    </button>
  {/if}

  <section>
    <button type="button" class="disclose" onclick={() => (advanced = !advanced)}>
      <span class="chev" class:open={advanced}><Icon name="chevron" size={12} stroke={2} /></span>
      Servers (IMAP &amp; SMTP)
    </button>
    {#if advanced}
      <div class="card servers-card">
        <div class="grid servers">
          <label>
            <span>Incoming (IMAP)</span>
            <input class="field" bind:value={draft.imapHost} oninput={() => (hostsTouched = true)} placeholder="imap.example.com" spellcheck="false" />
          </label>
          <label class="port">
            <span>Port</span>
            <input class="field" type="number" bind:value={draft.imapPort} />
          </label>
          <label>
            <span>Outgoing (SMTP)</span>
            <input class="field" bind:value={draft.smtpHost} oninput={() => (hostsTouched = true)} placeholder="smtp.example.com" spellcheck="false" />
          </label>
          <label class="port">
            <span>Port</span>
            <input class="field" type="number" bind:value={draft.smtpPort} />
          </label>
        </div>
        <p class="hint">SSL on 993 and 465. STARTTLS on 587.</p>
      </div>
    {/if}
  </section>

  {#if error}<div class="error">{error}</div>{/if}

  <footer>
    {#if draft.id}
      <button type="button" class="btn ghost danger" disabled={saving} onclick={remove}>Remove account</button>
    {/if}
    <span class="grow"></span>
    <button type="button" class="btn primary" disabled={saving} onclick={save} title="Save  ⌘S">
      {saving ? "Connecting…" : draft.id ? "Save changes" : "Connect account"}
    </button>
  </footer>
</div>

<style>
  .acc-pane { display: flex; flex-direction: column; gap: 18px; }
  .welcome { margin: -4px 0 2px; font-size: 13.5px; color: var(--muted); line-height: 1.5; }
  .card {
    padding: 14px;
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--panel);
  }
  .notice {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 12px;
    border-radius: 8px;
    background: var(--danger-soft);
    color: var(--danger);
    font-size: 13px;
    -webkit-user-select: text;
    user-select: text;
  }
  .notice :global(svg) { flex: none; }
  .notice-msg { flex: 1; min-width: 0; }
  .error {
    padding: 10px 12px;
    border-radius: 8px;
    background: var(--danger-soft);
    color: var(--danger);
    font-size: 13px;
    -webkit-user-select: text;
    user-select: text;
  }
  .grid { display: grid; grid-template-columns: 1fr 1fr; gap: 14px; }
  .wide { grid-column: span 2; }
  label { display: flex; flex-direction: column; gap: 6px; }
  label span, .lbl { font-size: 12.5px; font-weight: 500; color: var(--text-2); }
  .detected {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-top: 10px;
    padding-top: 10px;
    border-top: 1px solid var(--line);
    font-size: 12.5px;
  }
  .detected-ok { display: inline-flex; align-items: center; gap: 6px; color: var(--success); font-weight: 500; }
  .link { border: 0; padding: 0; background: none; color: var(--accent); font-size: 12.5px; font-weight: 500; cursor: pointer; }
  .link:hover { text-decoration: underline; }
  .swatches { display: flex; gap: 8px; margin-top: 4px; }
  .swatch {
    width: 22px;
    height: 22px;
    border-radius: 50%;
    border: 0;
    padding: 0;
    box-shadow: inset 0 0 0 1px rgba(0, 0, 0, 0.08);
    transition: transform 0.12s, box-shadow 0.12s;
  }
  .swatch:hover { transform: scale(1.1); }
  .swatch.on { box-shadow: 0 0 0 2px var(--panel), 0 0 0 4px var(--text); }
  section { display: flex; flex-direction: column; gap: 8px; }
  .sec-head { display: flex; align-items: center; justify-content: space-between; }
  .sec-title { font-size: 12px; font-weight: 600; color: var(--muted); }
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
  .seg button:hover:not(.on) { color: var(--text); }
  .seg button.on { background: var(--panel); color: var(--text); box-shadow: 0 1px 2px rgba(0, 0, 0, 0.08); }
  .sig {
    display: flex;
    flex-direction: column;
    min-height: 170px;
    padding: 10px 14px;
    border: 1px solid var(--line-strong);
    border-radius: var(--radius);
    background: var(--field);
  }
  .sig.html { padding: 0; }
  textarea {
    flex: 1;
    min-height: 170px;
    padding: 12px 14px;
    border: 0;
    border-radius: var(--radius);
    outline: none;
    background: transparent;
    resize: vertical;
    font: 12.5px/1.55 ui-monospace, "SF Mono", Menlo, monospace;
  }
  .preview-label { font-size: 12px; font-weight: 600; color: var(--muted); margin-top: 6px; }
  .preview { padding: 12px 14px; border-radius: var(--radius); background: #fff; border: 1px solid var(--line); }
  .disclose {
    display: inline-flex;
    align-items: center;
    align-self: flex-start;
    gap: 6px;
    border: 0;
    background: none;
    padding: 4px 6px;
    margin-left: -6px;
    border-radius: 6px;
    font-size: 13px;
    font-weight: 550;
    color: var(--text-2);
  }
  .disclose:hover { background: var(--hover); color: var(--text); }
  .chev { display: grid; color: var(--muted); transition: transform 0.15s; }
  .chev.open { transform: rotate(90deg); }
  .servers-card { margin-top: 2px; }
  .servers { grid-template-columns: 1fr 96px; }
  .hint { margin: 8px 0 0; font-size: 12px; color: var(--muted); }
  .hint.err { color: var(--danger); }
  .alias-add { display: grid; grid-template-columns: 1fr 1.2fr auto; gap: 8px; align-items: center; }
  .alias-list { display: flex; flex-direction: column; margin-top: 10px; border-top: 1px solid var(--line); }
  .alias-row { display: flex; align-items: center; justify-content: space-between; gap: 8px; padding: 7px 0; border-bottom: 1px solid var(--line); font-size: 13px; }
  .alias-row:last-child { border-bottom: 0; padding-bottom: 0; }
  .alias-info { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .alias-mail { color: var(--muted); }
  .icon-btn.sm { width: 26px; height: 26px; flex: none; }
  .grow { flex: 1; }
  footer {
    display: flex;
    align-items: center;
    gap: 8px;
    padding-top: 12px;
    border-top: 1px solid var(--line);
  }
</style>
