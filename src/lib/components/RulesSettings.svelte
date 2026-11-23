<script lang="ts">
  import { onMount } from "svelte";
  import { ask } from "@tauri-apps/plugin-dialog";
  import { api, type Rule, type RuleActivity } from "../api";
  import { sortable } from "../sortable";
  import { shortDate } from "../format";
  import Icon from "./Icon.svelte";
  import RuleEditor from "./RuleEditor.svelte";
  import { rulesUi, loadRules, ruleSummary, openRuleEditor, actionText, sends } from "../rules.svelte";
  import { describeRule, type RuleStep } from "../ai/rules";
  import { searchReady } from "../ai/config.svelte";
  import { openSettings, toast } from "../state.svelte";

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
    if (on && r.actions.some(sends)) {
      const who = r.needsApproval ? "An agent set up this rule. " : "";
      const ok = await ask(`${who}It sends mail in your name while Mailvern is open:\n\n${sendingLines(r)}`, {
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
</script>

{#if rulesUi.draft}
  <RuleEditor onsaved={loadActivity} />
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
      <button class="btn sm" disabled={!describe.trim()}>Create</button>
    {/if}
  </form>
  {#if rulesUi.source}
    <div class="source">
      <span>Like <b>{rulesUi.source.fromName || rulesUi.source.fromAddr}</b> · “{rulesUi.source.subject || "(no subject)"}”</span>
      <button class="icon-btn" title="Don't start from this message" onclick={() => (rulesUi.source = null)} disabled={thinking}><Icon name="x" size={13} /></button>
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
      <h3>Rules</h3>
      <button class="btn sm" onclick={() => openRuleEditor()}><Icon name="plus" size={14} /> New rule</button>
    </div>
    {#if rulesUi.loaded && !rulesUi.rules.length}
      <p class="empty">No rules yet. Rules file, flag, forward or answer new mail as it arrives.</p>
    {/if}
    <div class="list" use:sortable={{ item: ".rule", onsort: reorder }}>
      {#each rulesUi.rules as r (r.id)}
        <div class="rule" class:off={!r.enabled} data-id={r.id}>
          <input
            type="checkbox"
            role="switch"
            aria-label={`Turn “${r.name}” on`}
            checked={r.enabled}
            onchange={(e) => toggle(r, e.currentTarget.checked, e.currentTarget)} />
          <button class="body" onclick={() => openRuleEditor(r)}>
            <span class="name">
              {r.name}
              {#if r.needsApproval}<span class="badge warn">Needs approval</span>
              {:else if r.pausedReason}<span class="badge">Paused</span>{/if}
            </span>
            <span class="sum">{ruleSummary(r)}</span>
            {#if r.needsApproval}
              <span class="why">An agent changed what this rule sends. Check it, then turn it on.</span>
            {:else if r.pausedReason}
              <span class="why">Switched off because {r.pausedReason}.</span>
            {/if}
          </button>
          <Icon name="chevron" size={14} />
        </div>
      {/each}
    </div>
    {#if rulesUi.rules.length}
      <p class="hint">Rules run top to bottom while Mailvern is open. Drag to reorder.</p>
    {/if}
  </section>

  {#if activity.length}
    <section>
      <h3>Recent activity</h3>
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
              <button class="btn ghost sm" onclick={() => undo(a)}>Undo</button>
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
  .ghead { display: flex; align-items: center; justify-content: space-between; margin-bottom: 8px; }
  .btn.sm { height: 26px; padding: 0 10px; font-size: 12.5px; }
  .hint, .empty { margin: 8px 0 0; font-size: 12.5px; color: var(--muted); }
  .err { margin: 8px 0 0; font-size: 12.5px; color: var(--danger); }

  .describe {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 6px 0 11px;
    height: 40px;
    border: 1px solid var(--line-strong);
    border-radius: var(--radius);
    background: var(--field);
    transition: border-color 0.12s, box-shadow 0.12s;
  }
  .describe:focus-within { border-color: var(--accent); box-shadow: 0 0 0 3px var(--accent-soft); }
  .describe :global(svg) { color: var(--accent); flex: none; }
  .plain { flex: 1; min-width: 0; height: 100%; border: 0; outline: none; background: transparent; font-size: 13.5px; }
  .source { display: flex; align-items: center; gap: 6px; margin-top: 8px; padding: 2px 4px 2px 10px; border-radius: 8px; background: var(--hover); font-size: 12.5px; color: var(--text-2); }
  .source span { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .source .icon-btn { width: 24px; height: 24px; }
  .steps { margin: 8px 0 0; padding: 0 0 0 2px; list-style: none; display: flex; flex-direction: column; gap: 3px; font-size: 12.5px; color: var(--text-2); }
  .steps li.done { color: var(--muted); }
  .steps li::before { content: "•"; margin-right: 6px; color: var(--accent); }
  .steps li.done::before { content: "✓"; }

  .list { display: flex; flex-direction: column; border: 1px solid var(--line); border-radius: var(--radius); overflow: hidden; }
  .list:empty { display: none; }
  .rule { display: flex; align-items: center; gap: 12px; padding: 0 12px; background: var(--panel); color: var(--muted); }
  .rule + .rule { border-top: 1px solid var(--line); }
  .rule.off .name, .rule.off .sum { opacity: 0.6; }
  .body {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 10px 0;
    border: 0;
    background: transparent;
    text-align: left;
    color: var(--text);
  }
  .name { display: flex; align-items: center; gap: 8px; font-size: 13.5px; font-weight: 500; }
  .sum { font-size: 12.5px; color: var(--muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .why { font-size: 12px; color: var(--text-2); }
  .badge { font-size: 11px; font-weight: 600; padding: 1px 6px; border-radius: 5px; background: var(--hover); color: var(--muted); }
  .badge.warn { background: #fff1dc; color: #9a5b00; }
  @media (prefers-color-scheme: dark) {
    .badge.warn { background: rgba(255, 170, 60, 0.16); color: #ffbf66; }
  }

  .activity { display: flex; flex-direction: column; margin-top: 8px; }
  .act { display: flex; align-items: center; gap: 10px; padding: 7px 0; border-bottom: 1px solid var(--line); }
  .act.undone .what { opacity: 0.55; }
  .what { flex: 1; min-width: 0; display: flex; flex-wrap: wrap; column-gap: 6px; font-size: 13px; }
  .verb { font-weight: 500; }
  .subj { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; max-width: 100%; }
  .meta { width: 100%; font-size: 12px; color: var(--muted); }
  .act > .meta { width: auto; }

  /* A switch, drawn on the native checkbox (as in Agents). */
  input[role="switch"] {
    appearance: none;
    flex: none;
    position: relative;
    width: 30px;
    height: 18px;
    margin: 0;
    border-radius: 9px;
    background: var(--line-strong);
    transition: background 0.15s;
  }
  input[role="switch"]::after {
    content: "";
    position: absolute;
    top: 2px;
    left: 2px;
    width: 14px;
    height: 14px;
    border-radius: 50%;
    background: #fff;
    box-shadow: 0 1px 2px rgba(0, 0, 0, 0.2);
    transition: transform 0.15s;
  }
  input[role="switch"]:checked { background: var(--accent); }
  input[role="switch"]:checked::after { transform: translateX(12px); }
</style>
