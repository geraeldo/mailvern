<script lang="ts">
  import { onMount } from "svelte";
  import { ask } from "@tauri-apps/plugin-dialog";
  import { api, type Rule, type RuleActivity } from "../api";
  import { sortable } from "../sortable";
  import { shortDate } from "../format";
  import Icon from "./Icon.svelte";
  import Switch from "./Switch.svelte";
  import RuleEditor from "./RuleEditor.svelte";
  import { rulesUi, loadRules, conditionsSummary, actionsSummary, openRuleEditor, actionText, sends } from "../rules.svelte";
  import { describeRule, type RuleStep } from "../ai/rules";
  import { searchReady } from "../ai/config.svelte";
  import { openSettings, toast, accountById, accountLabel } from "../state.svelte";

  let { onback }: { onback?: () => void } = $props();

  let activity = $state<RuleActivity[]>([]);
  let describe = $state("");
  let thinking = $state(false);
  let aiError = $state<string | null>(null);
  let steps = $state<RuleStep[]>([]);
  let abort: AbortController | null = null;
  let describeInput = $state<HTMLInputElement>();

  // Coming from a message's "Describe a Rule…": ready for typing.
  $effect(() => {
    if (rulesUi.source) describeInput?.focus();
  });

  onMount(() => {
    loadRules();
    loadActivity();
    return () => abort?.abort();
  });

  async function loadActivity() {
    try {
      activity = await api.ruleActivity(30);
    } catch {
      activity = [];
    }
  }

  /** What a rule will send, in words, for the confirmation before it may. */
  function sendingLines(r: Rule): string {
    return r.actions
      .filter(sends)
      .map((a) => (a.type === "forward" ? `• Forward matching mail to ${a.to.join(", ")}` : a.type === "reply" ? `• Auto-reply: “${a.text.trim().slice(0, 140)}”` : ""))
      .join("\n");
  }

  async function toggle(r: Rule, on: boolean, input: HTMLInputElement) {
    if (on && (r.actions.some(sends) || r.needsApproval)) {
      const who = r.needsApproval ? "An agent created or changed this rule. " : "";
      const details = r.actions.some(sends)
        ? `It sends mail in your name while Mailvern is open:\n\n${sendingLines(r)}`
        : `It will act on incoming mail while Mailvern is open:\n\n${r.actions.map((a) => `• ${actionText(a)}`).join("\n")}`;
      const ok = await ask(`${who}${details}`, {
        title: `Turn on “${r.name}”?`,
        kind: "warning",
        okLabel: "Turn On",
      });
      if (!ok) {
        input.checked = false;
        return;
      }
    }
    try {
      await api.setRuleEnabled(r.id, on);
    } catch (e) {
      toast(String(e));
    }
    await loadRules();
  }

  async function reorder(ids: string[]) {
    const order = ids.map(Number);
    rulesUi.rules = order.map((id) => rulesUi.rules.find((r) => r.id === id)!).filter(Boolean);
    try {
      await api.reorderRules(order);
    } catch (e) {
      toast(String(e));
      loadRules();
    }
  }

  async function undo(a: RuleActivity) {
    try {
      await api.undoRuleActivity(a.id);
      toast("Undone");
    } catch (e) {
      toast(String(e), { ms: 5000 });
    }
    loadActivity();
  }

  async function fromText() {
    const text = describe.trim();
    if (!text || thinking) return;
    if (!searchReady()) {
      openSettings("ai");
      toast("Add an OpenRouter key and search model first");
      return;
    }
    thinking = true;
    aiError = null;
    steps = [];
    abort = new AbortController();
    try {
      const rule = await describeRule(text, { signal: abort.signal, source: $state.snapshot(rulesUi.source), onStep: (s) => (steps = s) });
      describe = "";
      openRuleEditor(rule);
    } catch (e) {
      if (!abort.signal.aborted) aiError = e instanceof Error ? e.message : String(e);
    }
    thinking = false;
    steps = [];
  }

  const summaryLabel = $derived.by(() => {
    const total = rulesUi.rules.length;
    if (!total) return "Your rules";
    const active = rulesUi.rules.filter((r) => r.enabled).length;
    const pending = rulesUi.rules.filter((r) => r.needsApproval).length;
    const parts = [`${active} of ${total} active`];
    if (pending) parts.push(`${pending} waiting for approval`);
    return parts.join(" · ");
  });
</script>

{#if rulesUi.draft}
  <RuleEditor onsaved={loadActivity} {onback} />
{:else}
  <form class="describe" onsubmit={(e) => { e.preventDefault(); fromText(); }}>
    <Icon name="sparkle" />
    <input
      class="plain"
      placeholder={rulesUi.source ? "What should happen to mail like this?" : "Describe a rule, e.g. “Archive GitHub mail and mark it read”"}
      bind:this={describeInput}
      bind:value={describe}
      disabled={thinking} />
    {#if thinking}
      <button type="button" class="btn ghost sm" onclick={() => abort?.abort()}>Stop</button>
    {:else}
      <button class="btn sm" class:primary={!!describe.trim()} disabled={!describe.trim()}>Create</button>
    {/if}
  </form>
  {#if rulesUi.source}
    <div class="source">
      <span>Like <b>{rulesUi.source.fromName || rulesUi.source.fromAddr}</b> · “{rulesUi.source.subject || "(no subject)"}”</span>
      <button type="button" class="icon-btn" title="Don't start from this message" onclick={() => (rulesUi.source = null)} disabled={thinking}><Icon name="x" size={13} /></button>
    </div>
  {/if}
  {#if thinking}
    <ul class="steps">
      {#each steps as s, i (i)}
        <li class:done={s.done}>{s.text}{s.done ? "" : "…"}</li>
      {/each}
    </ul>
  {/if}
  {#if aiError}<p class="err">{aiError}</p>{/if}

  <section>
    <div class="ghead">
      <h3>{summaryLabel}</h3>
      <button type="button" class="btn sm" onclick={() => openRuleEditor()}><Icon name="plus" size={14} /> New rule</button>
    </div>
    {#if rulesUi.loaded && !rulesUi.rules.length}
      <div class="empty-card">
        <p class="empty-title">No rules yet</p>
        <p class="empty">Describe what you want above, or build a rule manually to file, flag, forward, or answer new mail as it arrives.</p>
      </div>
    {:else}
      <div class="list" use:sortable={{ item: ".rule", onsort: reorder }}>
        {#each rulesUi.rules as r (r.id)}
          {@const acc = r.accountId != null ? accountById(r.accountId) : null}
          <div class="rule" class:off={!r.enabled} data-id={r.id}>
            <div class="lead-controls">
              {#if rulesUi.rules.length > 1}
                <span class="grip" title="Drag to reorder" aria-hidden="true"><Icon name="grip" size={14} /></span>
              {/if}
              <Switch checked={r.enabled} label={`Turn “${r.name}” on`} onchange={(on, input) => toggle(r, on, input)} />
            </div>
            <button type="button" class="body" onclick={() => openRuleEditor(r)}>
              <span class="text">
                <span class="title-row">
                  <span class="name">{r.name}</span>
                  {#if acc}
                    <span class="acc-chip" title={acc.email}>
                      <span class="acc-bar" style:background={acc.color || "var(--muted)"}></span>
                      {accountLabel(acc)}
                    </span>
                  {/if}
                  {#if r.needsApproval}
                    <span class="badge warn">Needs approval</span>
                  {:else if r.pausedReason}
                    <span class="badge">Paused</span>
                  {/if}
                  {#if r.stop}
                    <span class="badge">Stops</span>
                  {/if}
                </span>
                <span class="cond">{conditionsSummary(r)}</span>
                {#if r.actions.length}
                  <span class="acts">
                    <span class="arrow" aria-hidden="true">→</span>
                    <span class="acts-text">{actionsSummary(r)}</span>
                  </span>
                {/if}
                {#if r.needsApproval}
                  <span class="why">An agent created or changed this rule. Click to review and approve.</span>
                {:else if r.pausedReason}
                  <span class="why">Switched off because {r.pausedReason}.</span>
                {/if}
              </span>
              <span class="chev" aria-hidden="true"><Icon name="chevron" size={14} /></span>
            </button>
          </div>
        {/each}
      </div>
      {#if rulesUi.rules.length}
        <p class="hint">Rules run top to bottom while Mailvern is open.{rulesUi.rules.length > 1 ? " Drag to reorder." : ""}</p>
      {/if}
    {/if}
  </section>

  {#if activity.length}
    <section>
      <h3 class="act-head">Recent activity</h3>
      <div class="activity">
        {#each activity as a (a.id)}
          <div class="act" class:undone={a.undone}>
            <span class="what">
              <span class="verb">{actionText(a.action).replace(/^./, (c) => c.toUpperCase())}</span>
              <span class="subj">“{a.subject || "(no subject)"}”</span>
              <span class="meta">{a.fromAddr} · {a.ruleName} · {shortDate(a.at)}</span>
            </span>
            {#if a.undone}
              <span class="meta">Undone</span>
            {:else if a.canUndo}
              <button type="button" class="btn ghost sm" onclick={() => undo(a)}>Undo</button>
            {/if}
          </div>
        {/each}
      </div>
    </section>
  {/if}
{/if}

<style>
  section { margin-top: 22px; }
  h3 { margin: 0; font-size: 12px; font-weight: 600; color: var(--muted); }
  .act-head { margin-bottom: 8px; }
  .ghead { display: flex; align-items: center; justify-content: space-between; margin-bottom: 8px; }
  .hint { margin: 8px 0 0; font-size: 12px; color: var(--muted); }
  .err { margin: 8px 0 0; font-size: 12.5px; color: var(--danger); }

  .describe {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 6px 0 12px;
    height: 42px;
    border: 1px solid var(--line-strong);
    border-radius: var(--radius);
    background: var(--field);
    transition: border-color 0.12s, box-shadow 0.12s;
  }
  .describe:focus-within { border-color: var(--accent); box-shadow: 0 0 0 3px var(--accent-soft); }
  .describe :global(svg) { color: var(--accent); flex: none; }
  .plain { flex: 1; min-width: 0; height: 100%; border: 0; outline: none; background: transparent; font-size: 13.5px; }
  .source { display: flex; align-items: center; gap: 6px; margin-top: 8px; padding: 3px 4px 3px 10px; border-radius: 8px; background: var(--hover); font-size: 12.5px; color: var(--text-2); }
  .source span { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .source .icon-btn { width: 24px; height: 24px; }
  .steps { margin: 8px 0 0; padding: 0 0 0 2px; list-style: none; display: flex; flex-direction: column; gap: 3px; font-size: 12.5px; color: var(--text-2); }
  .steps li.done { color: var(--muted); }
  .steps li::before { content: "•"; margin-right: 6px; color: var(--accent); }
  .steps li.done::before { content: "✓"; }

  .empty-card {
    padding: 20px 18px;
    border: 1px dashed var(--line-strong);
    border-radius: var(--radius);
    text-align: center;
  }
  .empty-title { margin: 0 0 4px; font-size: 13.5px; font-weight: 500; color: var(--text-2); }
  .empty { margin: 0 auto; max-width: 420px; font-size: 12.5px; color: var(--muted); line-height: 1.5; }

  .list { display: flex; flex-direction: column; border: 1px solid var(--line); border-radius: var(--radius); background: var(--panel); }
  .list:empty { display: none; }
  .rule {
    display: flex;
    align-items: stretch;
    background: var(--panel);
    transition: background 0.12s;
  }
  .rule:first-child { border-radius: calc(var(--radius) - 1px) calc(var(--radius) - 1px) 0 0; }
  .rule:last-child { border-radius: 0 0 calc(var(--radius) - 1px) calc(var(--radius) - 1px); }
  .rule:only-child { border-radius: calc(var(--radius) - 1px); }
  .rule + .rule { border-top: 1px solid var(--line); }
  .rule:global([data-dragging]) {
    border-radius: var(--radius);
    box-shadow: var(--shadow);
    outline: 1px solid var(--line-strong);
  }
  .lead-controls {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 4px 0 10px;
    flex: none;
  }
  .grip {
    display: grid;
    place-items: center;
    color: var(--line-strong);
    cursor: grab;
    transition: color 0.12s;
  }
  .rule:hover .grip { color: var(--muted); }
  .rule.off .text { opacity: 0.58; }

  .body {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 11px 12px 11px 10px;
    border: 0;
    border-radius: 0;
    background: transparent;
    text-align: left;
    color: var(--text);
    transition: background 0.12s;
  }
  .rule:first-child .body { border-top-right-radius: calc(var(--radius) - 1px); }
  .rule:last-child .body { border-bottom-right-radius: calc(var(--radius) - 1px); }
  .body:hover { background: var(--hover); }
  .text {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .title-row {
    display: flex;
    align-items: center;
    gap: 7px;
    min-width: 0;
    max-width: 100%;
  }
  .name {
    font-size: 13.5px;
    font-weight: 550;
    color: var(--text);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }
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
    max-width: 120px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .acc-bar { width: 3px; height: 9px; border-radius: 2px; flex: none; }
  .cond {
    display: block;
    width: 100%;
    font-size: 12.5px;
    color: var(--muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .acts {
    display: flex;
    align-items: baseline;
    gap: 5px;
    min-width: 0;
    max-width: 100%;
    font-size: 12.5px;
    color: var(--text-2);
  }
  .arrow { color: var(--accent); font-weight: 600; flex: none; }
  .acts-text { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; font-weight: 500; }
  .why { font-size: 12px; color: var(--warn-text); margin-top: 2px; }
  .chev { display: grid; flex: none; color: var(--muted); transition: color 0.12s, transform 0.12s; }
  .body:hover .chev { color: var(--text); transform: translateX(1px); }

  .badge { flex: none; font-size: 11px; font-weight: 600; padding: 1px 6px; border-radius: 5px; background: var(--hover); color: var(--muted); }
  .badge.warn { background: var(--warn-soft); color: var(--warn-text); }

  .activity {
    display: flex;
    flex-direction: column;
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--panel);
    overflow: hidden;
  }
  .act { display: flex; align-items: center; gap: 10px; padding: 9px 12px; }
  .act + .act { border-top: 1px solid var(--line); }
  .act.undone .what { opacity: 0.55; }
  .what { flex: 1; min-width: 0; display: flex; flex-wrap: wrap; column-gap: 6px; font-size: 13px; }
  .verb { font-weight: 500; }
  .subj { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; max-width: 100%; }
  .meta { width: 100%; font-size: 12px; color: var(--muted); }
  .act > .meta { width: auto; }
</style>
