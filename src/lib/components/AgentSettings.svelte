<script lang="ts">
  import { onMount, tick } from "svelte";
  import { ask } from "@tauri-apps/plugin-dialog";
  import { api, type AgentAccess, type AgentAction, type AgentMode } from "../api";
  import { shortDate } from "../format";
  import { ui, accountById, accountLabel, loadPendingAgentActions, toast } from "../state.svelte";
  import Icon from "./Icon.svelte";
  import Select from "./Select.svelte";
  import Switch from "./Switch.svelte";

  // Access for coding agents on this Mac, through the MCP server in the app (crates/mail-mcp).
  let access = $state<AgentAccess | null>(null);
  let actions = $state<AgentAction[]>([]);
  let busy = $state(false);
  let actingId = $state<number | "all" | null>(null);
  let failed = $state<string | null>(null);
  let copied = $state<string | null>(null);
  let tab = $state("Claude Code");
  /** The token was just reset: every agent set up before is locked out until its command is run again. */
  let wasReset = $state(false);
  /** Agents whose new command was copied since the reset. */
  let redone = $state<string[]>([]);
  /** Shown after enabling automatic mode while the server is already running. */
  let reconnectHint = $state(false);
  let card = $state<HTMLElement>();
  let approvalRadio = $state<HTMLInputElement>();
  let autoRadio = $state<HTMLInputElement>();

  const pending = $derived(actions.filter((a) => a.status === "pending"));
  const recent = $derived(actions.filter((a) => a.status !== "pending"));

  onMount(() => {
    change(api.agentAccess());
    loadActions();
  });

  // Refresh the queue live when new actions arrive while Settings › Agent access is open.
  $effect(() => {
    ui.pendingAgentActions;
    loadActions();
  });

  async function loadActions() {
    try {
      actions = await api.agentActions(30);
    } catch {
      actions = [];
    }
  }

  async function change(next: Promise<AgentAccess>) {
    busy = true;
    failed = null;
    try {
      access = await next;
    } catch (e) {
      failed = String(e);
    }
    busy = false;
  }

  const effectiveMode = (accountId: number): AgentMode =>
    access?.accountModes[accountId] ?? access?.mode ?? "approval";

  const anyAuto = $derived.by(() => {
    if (!access) return false;
    if (!ui.accounts.length) return access.mode === "auto";
    return ui.accounts.some((a) => effectiveMode(a.id) === "auto");
  });

  const allAuto = $derived.by(() => {
    if (!access) return false;
    if (!ui.accounts.length) return access.mode === "auto";
    return access.mode === "auto" && ui.accounts.every((a) => effectiveMode(a.id) === "auto");
  });

  const toggleDetail = $derived(
    allAuto
      ? "Read, organize, and send mail, and manage rules automatically."
      : anyAuto
        ? "Permissions set per account. Sending and automatic changes only run where allowed."
        : "Read mail and save drafts. Changes to mail and rules wait for your approval.",
  );

  const footerNote = $derived(
    anyAuto
      ? "Sent mail waits for your Undo Send window before going out. Deleted mail moves to Trash."
      : "Drafts always wait for you to send. What an agent reads goes to its model provider.",
  );

  async function confirmAuto(scopeTitle: string): Promise<boolean> {
    return ask(
      "Connected agents will be able to send, move, archive, and trash mail, and activate rules without asking. Mail is untrusted text—only turn this on if you trust the agents you've connected.",
      {
        title: scopeTitle,
        kind: "warning",
        okLabel: "Allow",
        cancelLabel: "Cancel",
      },
    );
  }

  async function pickDefaultMode(mode: AgentMode) {
    if (!access || access.mode === mode || busy) return;
    const hadAuto = anyAuto;
    if (mode === "auto") {
      const title = ui.accounts.length > 1 ? "Let agents act automatically by default?" : "Let agents act automatically?";
      if (!(await confirmAuto(title))) {
        if (approvalRadio) approvalRadio.checked = access.mode === "approval";
        if (autoRadio) autoRadio.checked = access.mode === "auto";
        return;
      }
    }
    await change(api.setAgentMode(mode));
    if (!failed && !hadAuto && anyAuto && access?.running) reconnectHint = true;
    if (!anyAuto) reconnectHint = false;
  }

  const accountModeOptions = $derived<{ value: "default" | AgentMode; label: string; detail?: string }[]>([
    { value: "default", label: "Default", detail: access?.mode === "auto" ? "Run automatically" : "Ask before acting" },
    { value: "approval", label: "Ask before acting" },
    { value: "auto", label: "Run automatically" },
  ]);

  async function pickAccountMode(accountId: number, choice: "default" | AgentMode) {
    if (!access || busy) return;
    const prevEffective = effectiveMode(accountId);
    const nextEffective: AgentMode = choice === "default" ? access.mode : choice;
    const hadAuto = anyAuto;
    if (prevEffective !== "auto" && nextEffective === "auto") {
      const acc = accountById(accountId);
      const label = acc ? accountLabel(acc) : "this account";
      if (!(await confirmAuto(`Let agents act automatically on “${label}”?`))) return;
    }
    await change(api.setAgentAccountMode(accountId, choice === "default" ? null : choice));
    if (!failed && !hadAuto && anyAuto && access?.running) reconnectHint = true;
    if (!anyAuto) reconnectHint = false;
  }

  async function approve(a: AgentAction) {
    if (actingId !== null) return;
    actingId = a.id;
    try {
      await api.approveAgentAction(a.id);
      toast(`Approved: ${a.summary}`);
    } catch (e) {
      toast(String(e), { ms: 6000 });
    }
    actingId = null;
    await Promise.all([loadActions(), loadPendingAgentActions()]);
  }

  async function reject(a: AgentAction) {
    if (actingId !== null) return;
    actingId = a.id;
    try {
      await api.rejectAgentAction(a.id);
    } catch (e) {
      toast(String(e));
    }
    actingId = null;
    await Promise.all([loadActions(), loadPendingAgentActions()]);
  }

  async function approveAll() {
    if (actingId !== null || !pending.length) return;
    actingId = "all";
    try {
      const res = await api.approveAllAgentActions();
      if (res.failed > 0) {
        toast(`Approved ${res.succeeded}, ${res.failed} failed: ${res.errors[0] ?? ""}`, { ms: 6000 });
      } else {
        toast(res.succeeded === 1 ? "Approved 1 action" : `Approved ${res.succeeded} actions`);
      }
    } catch (e) {
      toast(String(e), { ms: 6000 });
    }
    actingId = null;
    await Promise.all([loadActions(), loadPendingAgentActions()]);
  }

  async function rejectAll() {
    if (actingId !== null || !pending.length) return;
    actingId = "all";
    try {
      const n = await api.rejectAllAgentActions();
      toast(n === 1 ? "Rejected 1 action" : `Rejected ${n} actions`);
    } catch (e) {
      toast(String(e));
    }
    actingId = null;
    await Promise.all([loadActions(), loadPendingAgentActions()]);
  }

  function actionIcon(kind: string): string {
    switch (kind) {
      case "archive": return "archive";
      case "trash": return "trash";
      case "move": return "move";
      case "flag":
      case "unflag": return "flag";
      case "markRead": return "mailOpen";
      case "markUnread": return "unread";
      case "junk":
      case "notJunk": return "junk";
      case "snooze":
      case "unsnooze": return "clock";
      case "deleteDraft": return "drafts";
      case "sendMail": return "sent";
      case "createRule":
      case "updateRule":
      case "deleteRule": return "filter";
      default: return "code";
    }
  }

  const isDestructive = (kind: string) => kind === "trash" || kind === "deleteDraft" || kind === "deleteRule";

  function statusLabel(status: AgentAction["status"]): string {
    switch (status) {
      case "approved": return "Approved";
      case "auto": return "Auto";
      case "rejected": return "Rejected";
      case "failed": return "Failed";
      default: return "Pending";
    }
  }

  const token = $derived(access?.token && /^[A-Za-z0-9_-]+$/.test(access.token) ? access.token : "");
  const header = $derived(`Authorization: Bearer ${token}`);
  const shQuote = (s: string) => `'${s.replaceAll("'", "'\\''")}'`;
  const commands = $derived<Record<string, string>>(
    access && token
      ? {
          "Claude Code": `claude mcp remove mailvern -s user >/dev/null 2>&1; claude mcp add --transport http --scope user mailvern ${shQuote(access.url)} --header ${shQuote(`Authorization: Bearer ${token}`)}`,
          Codex: `codex mcp remove mailvern >/dev/null 2>&1; mkdir -p ~/.codex && printf '\\n[mcp_servers.mailvern]\\nurl = "%s"\\nhttp_headers = { Authorization = "Bearer %s" }\\n' ${shQuote(access.url)} ${shQuote(token)} >> ~/.codex/config.toml`,
          OpenCode: `opencode mcp add --global mailvern --url ${shQuote(access.url)} --header ${shQuote(`Authorization=Bearer ${token}`)}`,
        }
      : {},
  );
  const tabs = ["Claude Code", "Codex", "OpenCode", "Other"];
  /** The token stays off the screen; Copy gives the real thing. */
  const masked = (s: string) => (token ? s.replaceAll(token, "••••••••") : s);

  async function copy(text: string, what: string, agent = what) {
    try {
      await api.copyText(text);
      copied = what;
      if (wasReset && !redone.includes(agent)) redone.push(agent);
      setTimeout(() => copied === what && (copied = null), 1600);
    } catch (e) {
      failed = String(e);
    }
  }

  async function reset() {
    const ok = await ask("Connected agents stop working until you rerun their command.", {
      title: "Reset the access token?",
      kind: "warning",
      okLabel: "Reset",
      cancelLabel: "Cancel",
    });
    if (!ok) return;
    await change(api.resetAgentToken());
    if (failed) return;
    wasReset = true;
    redone = [];
    await tick();
    card?.scrollIntoView({ behavior: "smooth", block: "nearest" });
  }
</script>

{#if pending.length}
  <section>
    <div class="ghead">
      <h3>Waiting for approval · {pending.length}</h3>
      {#if pending.length > 1}
        <div class="bulk-btns">
          <button type="button" class="btn ghost sm" disabled={actingId !== null} onclick={rejectAll}>Reject all</button>
          <button type="button" class="btn sm primary" disabled={actingId !== null} onclick={approveAll}>Approve all</button>
        </div>
      {/if}
    </div>
    <div class="queue-card">
      {#each pending as a (a.id)}
        {@const acc = a.accountId != null ? accountById(a.accountId) : null}
        {@const msgs = a.preview.messages ?? []}
        <div class="qrow">
          <span class="qicon" class:danger={isDestructive(a.kind)}><Icon name={actionIcon(a.kind)} size={15} /></span>
          <div class="qbody">
            <div class="qhead">
              <span class="qtitle">{a.summary}</span>
              {#if ui.accounts.length > 1}
                {#if acc}
                  <span class="acc-chip" title={acc.email}>
                    <span class="acc-bar" style:background={acc.color || "var(--muted)"}></span>
                    {accountLabel(acc)}
                  </span>
                {:else if a.accountId === null && (a.kind === "createRule" || a.kind === "updateRule" || a.kind === "deleteRule")}
                  <span class="acc-chip">All accounts</span>
                {/if}
              {/if}
            </div>
            {#if msgs.length}
              <ul class="qlist">
                {#each msgs.slice(0, 3) as m (m.id)}
                  <li>
                    <span class="qfrom">{m.fromName || m.fromAddr}</span>
                    <span class="qsubj">“{m.subject || "(no subject)"}”</span>
                  </li>
                {/each}
                {#if msgs.length > 3}
                  <li class="qmore">+{msgs.length - 3} more</li>
                {/if}
              </ul>
            {:else if a.preview.detail}
              <p class="qdetail">{a.preview.detail}</p>
            {/if}
            <span class="qtime">{shortDate(a.createdAt)}</span>
          </div>
          <div class="qacts">
            <button type="button" class="btn ghost sm" disabled={actingId !== null} onclick={() => reject(a)}>Reject</button>
            <button
              type="button"
              class="btn sm"
              class:primary={!isDestructive(a.kind)}
              class:danger={isDestructive(a.kind)}
              disabled={actingId !== null}
              onclick={() => approve(a)}>Approve</button>
          </div>
        </div>
      {/each}
    </div>
  </section>
{/if}

<section>
  <div class="toggle-card">
    <label class="toggle">
      <span class="text">
        <span class="label">
          Let agents on this Mac use Mailvern
          {#if access?.running}<span class="live"><span class="live-dot"></span>Running</span>{/if}
        </span>
        <span class="detail">{toggleDetail}</span>
      </span>
      <Switch
        checked={access?.enabled ?? false}
        disabled={busy || !access}
        label="Let agents on this Mac use Mailvern"
        onchange={(on) => change(api.setAgentAccess(on))} />
    </label>
  </div>
  {#if failed}
    <p class="err">{failed}</p>
  {:else if access?.enabled && access.error}
    <p class="err">Couldn't start: {access.error}</p>
  {/if}
</section>

{#if access?.enabled}
  <section>
    <h3>{ui.accounts.length > 1 ? "Permissions · Default for accounts" : "Permissions"}</h3>
    <div class="list-card">
      <label class="row interactive">
        <input
          bind:this={approvalRadio}
          type="radio"
          name="mcp-mode"
          value="approval"
          checked={access.mode === "approval"}
          disabled={busy}
          onchange={() => pickDefaultMode("approval")} />
        <span class="text">
          <span class="label">Ask before acting</span>
          <span class="detail">Agents can read mail and save drafts. Changes to mail and rules wait for your approval; sending is off.</span>
        </span>
      </label>
      <label class="row interactive">
        <input
          bind:this={autoRadio}
          type="radio"
          name="mcp-mode"
          value="auto"
          checked={access.mode === "auto"}
          disabled={busy}
          onchange={() => pickDefaultMode("auto")} />
        <span class="text">
          <span class="label">Run automatically</span>
          <span class="detail">Agents can organize and send mail, and turn on rules right away without asking.</span>
        </span>
      </label>
    </div>

    {#if ui.accounts.length > 1}
      <h3 class="subhead">By account</h3>
      <div class="list-card">
        {#each ui.accounts as acc (acc.id)}
          {@const override = access.accountModes[acc.id]}
          {@const val = (override ?? "default") as "default" | AgentMode}
          <div class="row acc-row">
            <span class="acc-bar lg" style:background={acc.color || "var(--muted)"}></span>
            <span class="text">
              <span class="label">{accountLabel(acc)}</span>
              <span class="detail">{acc.email}</span>
            </span>
            <Select
              label={`Permissions for ${accountLabel(acc)}`}
              value={val}
              options={accountModeOptions}
              onchange={(v) => pickAccountMode(acc.id, v)} />
          </div>
        {/each}
      </div>
    {/if}

    {#if reconnectHint}
      <p class="hint">Reconnect already-running agents if you want them to discover the sending tools.</p>
    {/if}
  </section>
{/if}

{#if access?.enabled && token}
  <section>
    <h3>Connect an agent</h3>
    <div class="card" class:reset={wasReset} bind:this={card}>
      {#if wasReset}
        <div class="banner">
          <Icon name="refresh" size={14} />
          <span>New token. Rerun the command for each agent.</span>
          <button type="button" class="btn ghost sm" onclick={() => (wasReset = false)}>Done</button>
        </div>
      {/if}

      <div class="seg" role="tablist">
        {#each tabs as t (t)}
          <button type="button" role="tab" aria-selected={tab === t} class:on={tab === t} onclick={() => (tab = t)}>
            {t}
            {#if wasReset}
              {#if redone.includes(t)}
                <span class="mark ok"><Icon name="check" size={11} stroke={2.5} /></span>
              {:else}
                <span class="mark due"></span>
              {/if}
            {/if}
          </button>
        {/each}
      </div>

      {#if tab === "Other"}
        <div class="field-row">
          <span class="key">URL</span>
          <code>{access.url}</code>
          <button type="button" class="icon-btn" title="Copy URL" onclick={() => copy(access!.url, "url", "Other")}>
            <Icon name={copied === "url" ? "check" : "copy"} size={14} />
          </button>
        </div>
        <div class="field-row">
          <span class="key">Header</span>
          <code>{masked(header)}</code>
          <button type="button" class="icon-btn" title="Copy header" onclick={() => copy(header, "header", "Other")}>
            <Icon name={copied === "header" ? "check" : "copy"} size={14} />
          </button>
        </div>
        <p class="hint">Add an HTTP MCP server with this URL and header.</p>
      {:else}
        <div class="cmd">
          <code>{masked(commands[tab])}</code>
          <button type="button" class="btn sm" onclick={() => copy(commands[tab], tab)}>
            <Icon name={copied === tab ? "check" : "copy"} size={13} />
            {copied === tab ? "Copied" : "Copy"}
          </button>
        </div>
        <p class="hint">Paste into Terminal and run.</p>
      {/if}

      <div class="foot">
        <span class="detail">Access token <code class="inline">{masked(token)}</code></span>
        <button type="button" class="btn ghost sm danger" disabled={busy} onclick={reset}>Reset token</button>
      </div>
    </div>
  </section>
{/if}

{#if recent.length}
  <section>
    <h3>Recent activity</h3>
    <div class="activity">
      {#each recent as a (a.id)}
        {@const acc = a.accountId != null ? accountById(a.accountId) : null}
        <div class="act" class:muted={a.status === "rejected"}>
          <span class="qicon sm"><Icon name={actionIcon(a.kind)} size={13} /></span>
          <span class="what">
            <span class="act-sum">{a.summary}</span>
            <span class="meta">
              {#if acc && ui.accounts.length > 1}{accountLabel(acc)} · {/if}
              {shortDate(a.resolvedAt ?? a.createdAt)}
              {#if a.error} · <span class="act-err">{a.error}</span>{/if}
            </span>
          </span>
          <span class="badge" class:ok={a.status === "approved" || a.status === "auto"} class:bad={a.status === "failed"}>
            {statusLabel(a.status)}
          </span>
        </div>
      {/each}
    </div>
  </section>
{/if}

<p class="note">{footerNote}</p>

<style>
  section + section { margin-top: 24px; }
  h3 { margin: 0 0 8px; font-size: 12px; font-weight: 600; color: var(--muted); }
  .subhead { margin-top: 14px; }
  .ghead { display: flex; align-items: center; justify-content: space-between; margin-bottom: 8px; }
  .ghead h3 { margin: 0; }
  .bulk-btns { display: flex; gap: 6px; }

  .toggle-card, .list-card, .queue-card, .activity {
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--panel);
    overflow: hidden;
  }
  .queue-card { border-color: color-mix(in srgb, var(--warn) 45%, var(--line)); }

  .toggle { display: flex; align-items: center; gap: 16px; padding: 12px 14px; cursor: default; }
  .toggle:hover { background: var(--hover); }
  .row { display: flex; align-items: center; gap: 12px; padding: 11px 14px; cursor: default; }
  .row + .row { border-top: 1px solid var(--line); }
  .row.interactive:hover { background: var(--hover); }
  .acc-row { padding: 9px 12px 9px 14px; }
  input[type="radio"] { margin: 0; accent-color: var(--accent); flex: none; }

  .text { flex: 1; display: flex; flex-direction: column; gap: 2px; min-width: 0; }
  .label { display: flex; align-items: center; flex-wrap: wrap; gap: 8px; font-size: 13.5px; color: var(--text); }
  .live {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 1px 7px;
    border-radius: 99px;
    background: color-mix(in srgb, var(--success) 14%, transparent);
    color: var(--success);
    font-size: 11px;
    font-weight: 600;
  }
  .live-dot { width: 6px; height: 6px; border-radius: 50%; background: var(--success); }
  .detail { font-size: 12.5px; color: var(--muted); }
  .err { margin: 8px 0 0; font-size: 12.5px; color: var(--danger); }

  .acc-chip {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 1px 6px;
    border-radius: 5px;
    background: var(--hover);
    color: var(--text-2);
    font-size: 11px;
    font-weight: 500;
    max-width: 140px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .acc-bar { width: 3px; height: 9px; border-radius: 2px; flex: none; }
  .acc-bar.lg { height: 22px; }

  .qrow { display: flex; align-items: flex-start; gap: 11px; padding: 11px 12px; }
  .qrow + .qrow { border-top: 1px solid var(--line); }
  .qicon {
    display: grid;
    place-items: center;
    width: 28px;
    height: 28px;
    border-radius: 7px;
    background: var(--hover);
    color: var(--text-2);
    flex: none;
    margin-top: 1px;
  }
  .qicon.danger { background: var(--danger-soft); color: var(--danger); }
  .qicon.sm { width: 24px; height: 24px; margin-top: 0; }
  .qbody { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 3px; }
  .qhead { display: flex; align-items: center; flex-wrap: wrap; gap: 6px; }
  .qtitle { font-size: 13.5px; font-weight: 550; color: var(--text); }
  .qlist { margin: 2px 0 0; padding: 0; list-style: none; display: flex; flex-direction: column; gap: 2px; font-size: 12px; color: var(--text-2); }
  .qlist li { display: flex; gap: 5px; min-width: 0; }
  .qfrom { font-weight: 500; flex: none; max-width: 140px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .qsubj { color: var(--muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; }
  .qmore { color: var(--muted); }
  .qdetail { margin: 1px 0 0; font-size: 12.5px; color: var(--text-2); line-height: 1.4; }
  .qtime { font-size: 11.5px; color: var(--muted); }
  .qacts { display: flex; gap: 6px; flex: none; align-self: center; }

  .card {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 12px;
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--panel);
    transition: border-color 0.2s;
  }
  .card.reset { border-color: var(--accent); }

  .banner {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: -4px -4px 0;
    padding: 6px 6px 6px 10px;
    border-radius: 7px;
    background: var(--accent-soft);
    color: var(--accent);
    font-size: 12.5px;
    animation: fade 0.2s;
  }
  .banner span { flex: 1; color: var(--text); }

  .seg { display: flex; align-self: flex-start; padding: 2px; border-radius: 8px; background: var(--hover); }
  .seg button {
    display: inline-flex;
    align-items: center;
    gap: 5px;
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
  .mark { display: inline-grid; place-items: center; }
  .mark.due { width: 6px; height: 6px; border-radius: 50%; background: var(--accent); }
  .mark.ok { color: var(--accent); }

  code {
    font: 11.5px/1.55 ui-monospace, "SF Mono", Menlo, monospace;
    color: var(--text-2);
    overflow-wrap: anywhere;
    user-select: text;
    -webkit-user-select: text;
  }
  .cmd {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 8px 8px 8px 10px;
    border-radius: 8px;
    background: var(--hover);
  }
  .cmd code { flex: 1; min-width: 0; padding-top: 4px; }
  .field-row {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 4px 4px 4px 10px;
    border-radius: 8px;
    background: var(--hover);
  }
  .field-row code { flex: 1; min-width: 0; }
  .field-row + .field-row { margin-top: -4px; }
  .key { width: 46px; flex: none; font-size: 11.5px; color: var(--muted); }
  .hint { margin: 8px 0 0; font-size: 12px; color: var(--muted); }
  .card .hint { margin-top: -2px; }

  .foot {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-top: 2px;
    padding-top: 10px;
    border-top: 1px solid var(--line);
  }
  .foot .detail { display: flex; align-items: center; gap: 6px; }
  code.inline { letter-spacing: 1px; color: var(--muted); }

  .act { display: flex; align-items: center; gap: 10px; padding: 9px 12px; }
  .act + .act { border-top: 1px solid var(--line); }
  .act.muted .what { opacity: 0.55; }
  .what { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
  .act-sum { font-size: 13px; color: var(--text); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .meta { font-size: 12px; color: var(--muted); }
  .act-err { color: var(--danger); }
  .badge { flex: none; font-size: 11px; font-weight: 600; padding: 1px 6px; border-radius: 5px; background: var(--hover); color: var(--muted); }
  .badge.ok { background: color-mix(in srgb, var(--success) 14%, transparent); color: var(--success); }
  .badge.bad { background: var(--danger-soft); color: var(--danger); }

  .note { margin: 16px 0 0; font-size: 12px; line-height: 1.5; color: var(--muted); }
</style>
