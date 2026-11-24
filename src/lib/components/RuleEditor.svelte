<script lang="ts">
  import { ask } from "@tauri-apps/plugin-dialog";
  import { api, type Rule, type RuleAction, type RuleCondition, type RuleTest } from "../api";
  import Icon from "./Icon.svelte";
  import Select from "./Select.svelte";
  import Switch from "./Switch.svelte";
  import RecipientField from "./RecipientField.svelte";
  import { rulesUi, loadRules, FIELDS, OPS, ACTIONS, SNOOZES, isFlag, blankAction, sends, relocates } from "../rules.svelte";
  import { ui, accountLabel, toast } from "../state.svelte";

  let { onsaved, onback }: { onsaved?: () => void; onback?: () => void } = $props();

  // The draft lives in rulesUi so "Create rule…" and the AI can fill it before this opens.
  const r = $derived(rulesUi.draft!);
  /** The rule as saved, to tell what changed. */
  const saved = $derived(rulesUi.rules.find((x) => x.id === r.id) ?? null);

  let error = $state<string | null>(null);
  let test = $state<RuleTest | null>(null);
  let testing = $state(false);
  let testError = $state<string | null>(null);
  let busy = $state(false);

  const accountOptions = $derived([
    { value: null as number | null, label: "All accounts" },
    ...ui.accounts.map((a) => ({ value: a.id as number | null, label: accountLabel(a) })),
  ]);
  const folderOptions = $derived(
    ui.folders
      .filter((f) => f.accountId === r.accountId && f.role !== "inbox" && f.role !== "snoozed")
      .map((f) => ({ value: f.id, label: f.name })),
  );
  const fieldOptions = FIELDS.map((f) => ({ value: f.value, label: f.label }));

  /** Actions that can still go next to the others: each once, and one place to put the message. */
  function actionChoices(others: RuleAction[]) {
    const used = new Set(others.map((a) => a.type));
    const placed = others.some(relocates);
    return ACTIONS.filter((a) => !used.has(a.value) && !(placed && relocates({ type: a.value } as RuleAction)));
  }
  const addable = $derived(actionChoices(r.actions));

  const placeholder: Record<string, string> = {
    from: "name or address",
    fromDomain: "example.com",
    to: "name or address",
    cc: "name or address",
    toOrCc: "name or address",
    subject: "words in the subject",
    body: "words in the message",
    listId: "list name or id",
  };

  const blank = (c: RuleCondition) => !isFlag(c.field) && !c.value.trim();
  /** A condition still waiting for its words: nothing to preview or save yet. */
  const unfinished = $derived(r.conditions.some(blank) || r.exceptions.some(blank));
  const tooManyRelocations = $derived(r.actions.filter(relocates).length > 1);
  const moveNeedsAccount = $derived(r.actions.some((a) => a.type === "move") && r.accountId == null);
  const blockReason = $derived(
    !r.actions.length
      ? "Add at least one action"
      : unfinished
        ? "Fill in all condition fields"
        : tooManyRelocations
          ? "Keep only one destination action"
          : moveNeedsAccount
            ? "Pick an account to choose a folder"
            : "",
  );

  // Live preview: how many recent Inbox messages this would catch. Debounced, and only the
  // newest answer counts. Keep previous preview softly visible while refreshing to avoid layout shift.
  let seq = 0;
  $effect(() => {
    const snapshot = JSON.stringify(r.conditions) + JSON.stringify(r.exceptions) + r.matchAll + r.accountId;
    void snapshot;
    const mine = ++seq;
    testError = null;
    if (!r.conditions.length || unfinished) {
      test = null;
      testing = false;
      return;
    }
    testing = true;
    const rule = $state.snapshot(r) as Rule;
    const t = setTimeout(async () => {
      try {
        const res = await api.testRule({ ...rule, name: rule.name || "Preview", actions: [{ type: "markRead" }] });
        if (mine === seq) test = res;
      } catch (e) {
        if (mine === seq) testError = String(e);
      } finally {
        if (mine === seq) testing = false;
      }
    }, 350);
    return () => clearTimeout(t);
  });

  /** Set when a row is added, so its first field takes the cursor. */
  let focusNew = false;
  function autofocus(node: HTMLElement) {
    if (!focusNew) return;
    focusNew = false;
    queueMicrotask(() => node.focus());
  }

  function addCondition(list: RuleCondition[], field: RuleCondition["field"]) {
    focusNew = !isFlag(field);
    list.push({ field, op: "contains", value: "" });
  }

  function setField(c: RuleCondition, field: RuleCondition["field"]) {
    c.field = field;
    if (isFlag(field)) c.value = "";
  }

  function addAction(type: RuleAction["type"]) {
    focusNew = type === "reply";
    r.actions.push(blankAction(type, r.accountId));
  }

  function setAction(i: number, type: RuleAction["type"]) {
    r.actions[i] = blankAction(type, r.accountId);
  }

  function setAccount(id: number | null) {
    r.accountId = id;
    // A folder belongs to one account: point Move at one of the new account's folders.
    r.actions = r.actions.map((a) => (a.type === "move" ? blankAction("move", id) : a));
  }

  const actionsSend = (rule: Rule | null) => JSON.stringify((rule?.actions ?? []).filter(sends));

  function sendingLines(rule: Rule): string {
    return rule.actions
      .filter(sends)
      .map((a) => (a.type === "forward" ? `• Forward matching mail to ${a.to.join(", ")}` : a.type === "reply" ? `• Auto-reply: “${a.text.trim().slice(0, 140)}”` : ""))
      .join("\n");
  }

  /** Saves the draft. Returns the saved rule, or null if it wasn't. */
  async function save(close = true, forceEnable = false): Promise<Rule | null> {
    error = null;
    const rule = $state.snapshot(r) as Rule;
    if (forceEnable) rule.enabled = true;
    rule.name = rule.name.trim() || autoName(rule);
    // Mail sent in your name: say exactly what, before it can happen.
    const newSending = rule.actions.some(sends) && (actionsSend(rule) !== actionsSend(saved) || saved?.needsApproval);
    if (rule.enabled && newSending) {
      const ok = await ask(`It sends mail in your name while Mailvern is open:\n\n${sendingLines(rule)}`, {
        title: `Save and turn on “${rule.name}”?`,
        kind: "warning",
        okLabel: forceEnable ? "Approve & Turn On" : "Save",
      });
      if (!ok) return null;
    }
    busy = true;
    try {
      const out = await api.saveRule(rule);
      await loadRules();
      onsaved?.();
      if (close) {
        rulesUi.draft = null;
        rulesUi.draftOrigin = "";
      } else {
        rulesUi.draft = out;
        rulesUi.draftOrigin = JSON.stringify(out);
      }
      return out;
    } catch (e) {
      error = String(e);
      return null;
    } finally {
      busy = false;
    }
  }

  function autoName(rule: Rule): string {
    const c = rule.conditions[0];
    if (!c) return "All new mail";
    const label = FIELDS.find((f) => f.value === c.field)!.label;
    if (isFlag(c.field) || !c.value.trim()) return label;
    return `${label}: ${c.value.trim()}`.slice(0, 60);
  }

  async function remove() {
    const ok = await ask(`Delete “${r.name || "this rule"}”? Mail it already handled stays where it is.`, {
      title: "Delete rule?",
      kind: "warning",
      okLabel: "Delete",
    });
    if (!ok) return;
    try {
      await api.deleteRule(r.id);
      rulesUi.draft = null;
      rulesUi.draftOrigin = "";
      await loadRules();
    } catch (e) {
      error = String(e);
    }
  }

  async function runNow() {
    const rule = await save(false);
    if (!rule) return;
    const t = await api.testRule(rule).catch(() => null);
    const n = t?.count ?? 0;
    if (!n) {
      toast("Nothing in the Inbox matches");
      return;
    }
    const where = rule.accountId == null ? "your Inboxes" : "the Inbox";
    if (!(await ask(`Apply “${rule.name}” to ${n} message${n === 1 ? "" : "s"} already in ${where}?`, { title: "Run rule now?", okLabel: "Run" }))) return;
    let includeSending = false;
    if (rule.actions.some(sends)) {
      includeSending = await ask(`Also send for each of them?\n\n${sendingLines(rule)}\n\nAt most 50 an hour. Mailing lists and automatic mail are skipped.`, {
        title: `Send for ${n} existing message${n === 1 ? "" : "s"}?`,
        kind: "warning",
        okLabel: "Send Too",
        cancelLabel: "Don't Send",
      });
    }
    busy = true;
    try {
      const res = await api.runRule(rule.id, null, includeSending);
      const sent = res.sent ? `, sent ${res.sent}` : "";
      toast(`“${rule.name}” handled ${res.matched} message${res.matched === 1 ? "" : "s"}${sent}`);
      onsaved?.();
    } catch (e) {
      error = String(e);
    }
    busy = false;
  }

  const where = $derived(r.accountId == null ? "your Inboxes" : "the Inbox");
  /** "Stop here" only means something when other rules come after. */
  const showStop = $derived(r.stop || rulesUi.rules.some((x) => x.id !== r.id));
</script>

<div class="editor">
  <div class="top">
    <input class="field name" placeholder={r.conditions.length ? autoName(r) : "Rule name"} bind:value={r.name} aria-label="Rule name" />
    {#if !r.needsApproval}
      <label class="active-toggle">
        <span>Active</span>
        <Switch bind:checked={r.enabled} label="Rule active" />
      </label>
    {/if}
  </div>

  {#if r.needsApproval}
    <p class="banner warn">An agent proposed this rule. Review the conditions and actions below, then approve to turn it on.</p>
  {:else if r.pausedReason}
    <p class="banner warn">Switched off because {r.pausedReason}.</p>
  {/if}

  <section class="block">
    <div class="line">
      <span class="lead">When new mail arrives in</span>
      <Select label="Account" value={r.accountId} options={accountOptions} onchange={setAccount} />
      {#if r.conditions.length > 1}
        <span class="lead">and</span>
        <Select
          label="Match"
          value={r.matchAll}
          options={[{ value: true, label: "all of these hold" }, { value: false, label: "any of these holds" }]}
          onchange={(v) => (r.matchAll = v)} />
      {/if}
    </div>
    {#each r.conditions as c, i (i)}
      {@render condition(c, () => r.conditions.splice(i, 1))}
    {/each}
    {#if !r.conditions.length && r.actions.length}<p class="hint">No conditions, so it acts on every new message.</p>{/if}
    <div class="adds">
      <Select prompt="Condition" label="Add a condition" value={null} options={fieldOptions} onchange={(f) => addCondition(r.conditions, f!)} />
      {#if r.conditions.length && !r.exceptions.length}
        <Select prompt="Exception" label="Add an exception" value={null} options={fieldOptions} onchange={(f) => addCondition(r.exceptions, f!)} />
      {/if}
    </div>
  </section>

  {#if r.exceptions.length}
    <section class="block">
      <div class="lead head">Except when any of these holds</div>
      {#each r.exceptions as c, i (i)}
        {@render condition(c, () => r.exceptions.splice(i, 1))}
      {/each}
      <div class="adds">
        <Select prompt="Exception" label="Add an exception" value={null} options={fieldOptions} onchange={(f) => addCondition(r.exceptions, f!)} />
      </div>
    </section>
  {/if}

  <section class="block">
    <div class="lead head">Then</div>
    {#each r.actions as a, i (i)}
      <div class="row">
        <Select
          label="Action"
          value={a.type}
          options={[ACTIONS.find((x) => x.value === a.type)!, ...actionChoices(r.actions.filter((_, j) => j !== i))]}
          onchange={(v) => setAction(i, v)} />
        {#if a.type === "move"}
          {#if r.accountId == null}
            <span class="note">Pick an account above to choose a folder</span>
          {:else}
            <Select label="Folder" value={a.folderId} options={folderOptions} onchange={(v) => (a.folderId = v)} />
            <span class="grow"></span>
          {/if}
        {:else if a.type === "snooze"}
          <Select label="Snooze until" value={a.until} options={SNOOZES} onchange={(v) => (a.until = v)} />
          <span class="grow"></span>
        {:else if a.type === "forward"}
          <div class="grow recipients"><RecipientField label="Forward to" bind:value={a.to} /></div>
        {:else}
          <span class="grow"></span>
        {/if}
        <button type="button" class="icon-btn" title="Remove" onclick={() => r.actions.splice(i, 1)}><Icon name="x" size={14} /></button>
      </div>
      {#if a.type === "reply"}
        <div class="reply">
          <input class="field" placeholder="Subject (default: Re: their subject)" value={a.subject ?? ""} oninput={(e) => (a.subject = e.currentTarget.value || null)} />
          <textarea class="field" rows="4" placeholder="Your reply" bind:value={a.text} use:autofocus></textarea>
          <p class="hint">Once per sender every 4 days. Never to mailing lists, automatic mail or you.</p>
        </div>
      {/if}
      {#if a.type === "forward"}
        <p class="hint sub">Mail a rule already forwarded isn't forwarded again.</p>
      {/if}
    {/each}
    {#if tooManyRelocations}
      <p class="err">Keep only one of Move, Archive, Delete, Junk and Snooze.</p>
    {/if}
    {#if addable.length}
      <div class="adds">
        <Select prompt="Action" label="Add an action" value={null} options={addable} onchange={(t) => addAction(t!)} />
      </div>
    {/if}
    {#if showStop}
      <label class="check">
        <input type="checkbox" bind:checked={r.stop} />
        <span>Don't run later rules on mail this one matches</span>
      </label>
    {/if}
  </section>

  {#if test || testError || testing}
    <div class="preview" class:checking={testing && !!test}>
      {#if testError}
        <span class="err">{testError}</span>
      {:else if test}
        <div class="preview-head">
          <span>
            {#if test.count}Matches <b>{test.count}</b> of the last {test.scanned} messages in {where}.
            {:else}No match in the last {test.scanned} messages in {where}.{/if}
            {#if test.withoutBody}{test.withoutBody} not downloaded yet.{/if}
          </span>
          {#if testing}<span class="checking-lbl">Checking…</span>{/if}
        </div>
        {#if test.sample.length}
          <ul>
            {#each test.sample.slice(0, 4) as m (m.id)}
              <li><span class="from">{m.fromName || m.fromAddr}</span> {m.subject || "(no subject)"}</li>
            {/each}
          </ul>
        {/if}
      {:else}
        <span class="checking-lbl">Checking recent mail…</span>
      {/if}
    </div>
  {/if}

  {#if error}<p class="err">{error}</p>{/if}

  <div class="foot">
    {#if r.id}
      <button type="button" class="btn ghost danger" disabled={busy} onclick={remove}>Delete</button>
      <button type="button" class="btn ghost" disabled={busy || !!blockReason} onclick={runNow}>Run on Inbox now…</button>
    {/if}
    <span class="grow"></span>
    <button type="button" class="btn" disabled={busy} onclick={() => (onback ? onback() : (rulesUi.draft = null))}>Cancel</button>
    {#if r.needsApproval}
      <button type="button" class="btn primary" disabled={busy || !!blockReason} title={blockReason} onclick={() => save(true, true)}>Approve &amp; turn on</button>
    {:else}
      <button type="button" class="btn primary" disabled={busy || !!blockReason} title={blockReason} onclick={() => save()}>Save</button>
    {/if}
  </div>
</div>

{#snippet condition(c: RuleCondition, remove: () => void)}
  <div class="row">
    <Select label="Field" value={c.field} options={fieldOptions} onchange={(v) => setField(c, v)} />
    {#if !isFlag(c.field)}
      <Select label="Comparison" value={c.op} options={OPS} onchange={(v) => (c.op = v)} />
      <input
        class="field grow"
        class:mono={c.op === "regex"}
        placeholder={c.op === "regex" ? "pattern, e.g. invoice|receipt" : placeholder[c.field]}
        bind:value={c.value}
        use:autofocus />
    {:else}
      <span class="grow"></span>
    {/if}
    <button type="button" class="icon-btn" title="Remove" onclick={remove}><Icon name="x" size={14} /></button>
  </div>
{/snippet}

<style>
  .editor { display: flex; flex-direction: column; gap: 16px; }
  .top { display: flex; align-items: center; gap: 14px; }
  .name { flex: 1; min-width: 0; font-size: 14.5px; font-weight: 550; }
  .active-toggle {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
    color: var(--text-2);
    cursor: default;
  }
  .block {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 12px 14px;
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--panel);
  }
  .line { display: flex; align-items: center; flex-wrap: wrap; gap: 8px; }
  .lead { font-size: 13px; color: var(--text-2); }
  .head { font-weight: 600; font-size: 12px; color: var(--muted); }
  .row { display: flex; align-items: center; gap: 8px; }
  .row .field { height: 32px; }
  .grow { flex: 1; min-width: 0; }
  .mono { font-family: ui-monospace, "SF Mono", Menlo, monospace; font-size: 12.5px; }
  .adds { display: flex; gap: 4px; margin-left: -8px; }
  .reply { display: flex; flex-direction: column; gap: 6px; margin-left: 2px; }
  .reply .field { height: 32px; }
  .reply textarea.field { height: auto; padding: 8px 11px; resize: vertical; }
  .check { display: flex; align-items: center; gap: 8px; font-size: 13px; margin-top: 4px; color: var(--text-2); }
  .check input { margin: 0; accent-color: var(--accent); }
  .hint { margin: 0; font-size: 12px; color: var(--muted); }
  .hint.sub { margin-top: -4px; }
  .note { font-size: 12.5px; color: var(--muted); flex: 1; }
  .err { margin: 0; font-size: 12.5px; color: var(--danger); }
  .banner { margin: 0; padding: 10px 12px; border-radius: 8px; background: var(--accent-soft); font-size: 12.5px; color: var(--text); }
  .banner.warn { background: var(--warn-soft); color: var(--warn-text); }
  .preview {
    padding: 10px 12px;
    border-radius: var(--radius);
    background: var(--hover);
    font-size: 12.5px;
    color: var(--text-2);
    transition: opacity 0.15s;
  }
  .preview.checking { opacity: 0.65; }
  .preview-head { display: flex; align-items: center; justify-content: space-between; gap: 10px; }
  .checking-lbl { font-size: 12px; color: var(--muted); flex: none; }
  .preview ul { margin: 6px 0 0; padding: 0; list-style: none; display: flex; flex-direction: column; gap: 2px; }
  .preview li { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--muted); }
  .preview .from { color: var(--text); font-weight: 500; margin-right: 4px; }
  .foot { display: flex; align-items: center; gap: 8px; padding-top: 8px; border-top: 1px solid var(--line); }
  /* The composer's recipient field, drawn as a form field. */
  .recipients :global(.row) {
    min-height: 32px;
    padding: 2px 6px;
    border: 1px solid var(--line-strong);
    border-radius: 8px;
    background: var(--field);
  }
  .recipients :global(.row:focus-within) { border-color: var(--accent); box-shadow: 0 0 0 3px var(--accent-soft); }
  .recipients :global(.label) { display: none; }
  .recipients :global(input) { height: 26px; font-size: 13.5px; }
  .recipients :global(.chip) { height: 24px; }
</style>
