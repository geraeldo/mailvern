<script lang="ts">
  import { tick } from "svelte";
  import Icon from "./Icon.svelte";
  import { openMessage, aiRunning, type AiSearch, type AiTurn } from "../state.svelte";
  import { askFollowUp, runAiSearch, stopAiSearch } from "../ai/search.svelte";
  import { api, type Summary } from "../api";
  import { escapeHtml } from "../html";
  import { displayName, shortDate } from "../format";

  let { s }: { s: AiSearch } = $props();

  let followUp = $state("");
  let scroller = $state<HTMLDivElement>();
  let inputEl = $state<HTMLTextAreaElement>();
  /** Turns whose steps are shown. */
  let showSteps = $state<Record<number, boolean>>({});
  /** Cited messages, for the hover preview. */
  let rows = $state<Record<number, Summary>>({});
  let hover = $state<{ id: number; x: number; top: string; bottom: string } | null>(null);
  let copiedTurn = $state<number | null>(null);

  const running = $derived(aiRunning(s));

  $effect(() => {
    const want = s.turns.filter((t) => !t.running).flatMap((t) => t.sources).filter((id) => !rows[id]);
    if (!want.length) return;
    api.summaries(want).then((list) => {
      for (const r of list) rows[r.id] = r;
    });
  });

  // Keep the newest turn in view as it arrives.
  let turnCount = 0;
  $effect(() => {
    const n = s.turns.length;
    if (n === turnCount) return;
    turnCount = n;
    tick().then(() => scroller?.lastElementChild?.scrollIntoView({ block: "start", behavior: n > 1 ? "smooth" : "auto" }));
  });

  /** Safe HTML for answer text: **bold**, "- " lines, and [n] citations as small numbered links. */
  function render(text: string, turn: AiTurn): string {
    const cite = (n: number) => {
      const id = turn.sources[n - 1];
      // The word joiner keeps a citation on the same line as the word before it.
      return id == null ? "" : `⁠<button class="cite" data-id="${id}" aria-label="Source ${n}">${n}</button>`;
    };
    const lines = escapeHtml(text)
      // "hiring [1]." reads better as "hiring.¹"
      .replace(/((?:\s*\[\d+(?:\s*,\s*\d+)*\])+)([.,;:])/g, "$2$1")
      .replace(/\*\*(.+?)\*\*/g, "<b>$1</b>")
      .replace(/\s*\[(\d+(?:\s*,\s*\d+)*)\]/g, (_, list: string) => list.split(",").map((n) => cite(Number(n.trim()))).join(""))
      .replace(/(<\/button>)\s*\[(\d+)\]/g, (_, end, n) => `${end}${cite(Number(n))}`)
      // Keep citations on the same line as the word before them.
      .replace(/([^\s>]+)((?:⁠<button class="cite"[^>]*>\d+<\/button>)+)/g, '<span class="nb">$1$2</span>')
      .split("\n")
      .filter((l) => l.trim());
    return lines.map((l) => (/^\s*[-•*]\s+/.test(l) ? `<span class="li">${l.replace(/^\s*[-•*]\s+/, "")}</span>` : `<span class="p">${l}</span>`)).join("");
  }

  function onClick(e: MouseEvent) {
    const c = (e.target as Element).closest?.<HTMLElement>(".cite");
    if (c) {
      hover = null;
      openMessage(Number(c.dataset.id));
    }
  }

  function onOver(e: MouseEvent) {
    const c = (e.target as Element).closest?.<HTMLElement>(".cite");
    if (!c) return;
    const r = c.getBoundingClientRect();
    const x = Math.max(8, Math.min(r.left - 8, window.innerWidth - 312));
    const above = r.bottom + 140 > window.innerHeight && r.top > 140;
    hover = {
      id: Number(c.dataset.id),
      x,
      top: above ? "auto" : `${r.bottom + 6}px`,
      bottom: above ? `${window.innerHeight - r.top + 6}px` : "auto",
    };
  }

  function onOut(e: MouseEvent) {
    if ((e.target as Element).closest?.(".cite")) hover = null;
  }

  function resizeInput() {
    if (!inputEl) return;
    inputEl.style.height = "auto";
    inputEl.style.height = `${Math.min(120, inputEl.scrollHeight)}px`;
  }

  function send() {
    const q = followUp.trim();
    if (!q || running) return;
    followUp = "";
    if (inputEl) inputEl.style.height = "auto";
    askFollowUp(q);
  }

  async function copyTurn(t: AiTurn, i: number) {
    const parts = [
      t.answer,
      ...t.facts.map((f) => `${f.label}: ${f.value}`),
      t.details,
    ]
      .filter(Boolean)
      .map((p) => p.replace(/\s*\[\d+(?:\s*,\s*\d+)*\]/g, ""));
    try {
      await api.copyText(parts.join("\n\n"));
      copiedTurn = i;
      setTimeout(() => copiedTurn === i && (copiedTurn = null), 1600);
    } catch {}
  }

  function retry(i: number) {
    if (i === 0) runAiSearch(s.turns[0].question);
    else askFollowUp(s.turns[i].question);
  }

  /** What it's doing now: the step in progress, else the last one. */
  function status(t: AiTurn): string {
    const steps = [...t.steps].reverse();
    return steps.find((x) => !x.done)?.text ?? steps.find((x) => x.kind !== "note")?.text ?? "Thinking…";
  }
  const cost = (n: number) => (n <= 0 ? "" : n < 0.01 ? "under $0.01" : `$${n.toFixed(2)}`);
  const preview = $derived(hover ? rows[hover.id] : null);
</script>

<div class="answer-view">
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions, a11y_mouse_events_have_key_events -->
  <div class="scroll body" bind:this={scroller} onclick={onClick} onmouseover={onOver} onmouseout={onOut}>
    {#each s.turns as t, i (i)}
      <section class="turn" class:first={i === 0}>
        <div class="q">{t.question}</div>

        {#if t.running && !t.answer}
          <div class="status"><span class="spinner"></span><span>{status(t)}</span></div>
        {/if}

        {#if t.answer}
          <div class="lead">{@html render(t.answer, t)}</div>
        {/if}
        {#if t.facts.length}
          <dl class="facts">
            {#each t.facts as f, j (j)}
              <dt>{f.label}</dt>
              <dd>{@html render(f.value, t)}</dd>
            {/each}
          </dl>
        {/if}
        {#if t.details}
          <div class="details">{@html render(t.details, t)}</div>
        {/if}
        {#if t.error}
          <div class="error">
            <Icon name="alert" size={14} />
            <span>{t.error}</span>
            <button class="link" onclick={() => retry(i)}>Try again</button>
          </div>
        {/if}

        {#if !t.running}
          <div class="foot">
            {#if t.steps.length}
              <button type="button" class="link quiet" onclick={() => (showSteps[i] = !showSteps[i])}>
                How I found this <Icon name={showSteps[i] ? "chevronDown" : "chevron"} size={11} stroke={2} />
              </button>
            {/if}
            {#if t.answer}
              <button type="button" class="link quiet" onclick={() => copyTurn(t, i)}>
                <Icon name={copiedTurn === i ? "check" : "copy"} size={11} stroke={2} />
                {copiedTurn === i ? "Copied" : "Copy"}
              </button>
            {/if}
            {#if cost(t.cost)}<span>{cost(t.cost)}</span>{/if}
          </div>
          {#if showSteps[i]}
            <ol class="steps">
              {#each t.steps as st, j (j)}
                <li class:note={st.kind === "note"} class:err={st.error}>{st.text}</li>
              {/each}
            </ol>
          {/if}
        {/if}
      </section>
    {/each}
  </div>

  <div class="ask">
    <textarea
      bind:this={inputEl}
      bind:value={followUp}
      rows="1"
      placeholder={running ? "Working on it…" : "Ask a follow-up"}
      spellcheck="true"
      oninput={resizeInput}
      onkeydown={(e) => {
        if (e.key === "Enter" && !e.shiftKey) {
          e.preventDefault();
          send();
        }
      }}></textarea>
    {#if running}
      <button type="button" class="icon-btn" title="Stop" onclick={stopAiSearch}><Icon name="stop" size={15} /></button>
    {:else}
      <button type="button" class="icon-btn send" title="Ask  ↩" disabled={!followUp.trim()} onclick={send}><Icon name="sent" size={15} /></button>
    {/if}
  </div>
  <div class="model">{s.model}</div>

  {#if hover && preview}
    <div class="peek" style:left="{hover.x}px" style:top={hover.top} style:bottom={hover.bottom}>
      <div class="peek-head">
        <span class="peek-from">{displayName(preview.fromName, preview.fromAddr)}</span>
        <span class="peek-date">{shortDate(preview.date)}</span>
      </div>
      <div class="peek-subject">{preview.subject || "(no subject)"}</div>
      {#if preview.snippet}<div class="peek-snip">{preview.snippet}</div>{/if}
    </div>
  {/if}
</div>

<style>
  .answer-view { flex: 1; min-height: 0; display: flex; flex-direction: column; }
  .body { flex: 1; min-height: 0; padding: 6px 36px 24px; }
  .turn { max-width: 680px; margin: 0 auto; padding: 22px 0 18px; border-top: 1px solid var(--line); animation: fade 0.15s ease-out; }
  .turn.first { border-top: 0; padding-top: 8px; }
  .q { font-size: 13px; color: var(--muted); margin-bottom: 10px; -webkit-user-select: text; user-select: text; }
  .status { display: flex; align-items: center; gap: 9px; color: var(--text-2); font-size: 14px; }
  .status span:last-child { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .lead { font-size: 19px; line-height: 1.4; font-weight: 600; letter-spacing: -0.01em; color: var(--text); -webkit-user-select: text; user-select: text; }
  .facts {
    display: grid;
    grid-template-columns: max-content minmax(0, 1fr);
    gap: 7px 18px;
    margin: 16px 0 0;
    padding: 12px 14px;
    border-radius: 10px;
    background: var(--side);
    font-size: 13.5px;
    line-height: 1.45;
    -webkit-user-select: text;
    user-select: text;
  }
  dt { color: var(--muted); }
  dd { margin: 0; color: var(--text); overflow-wrap: anywhere; }
  .details { margin-top: 14px; font-size: 14.5px; line-height: 1.6; color: var(--text-2); -webkit-user-select: text; user-select: text; }
  .details :global(.p) { display: block; }
  .details :global(.p + .p), .details :global(.li + .p), .details :global(.p + .li) { margin-top: 6px; }
  .body :global(.li) { display: block; position: relative; padding-left: 14px; }
  .body :global(.li)::before { content: "•"; position: absolute; left: 2px; color: var(--muted); }
  .lead :global(.p), dd :global(.p) { display: inline; }
  .body :global(.cite) {
    display: inline-grid;
    place-items: center;
    min-width: 16px;
    height: 16px;
    margin-left: 3px;
    padding: 0 4px;
    border: 0;
    border-radius: 5px;
    background: var(--accent-soft);
    color: var(--accent);
    font-size: 10.5px;
    font-weight: 650;
    line-height: 1;
    vertical-align: 2px;
    cursor: pointer;
  }
  .lead :global(.cite) { vertical-align: 4px; }
  .body :global(.nb) { white-space: nowrap; }
  .body :global(.cite:hover) { background: var(--accent); color: #fff; }
  .error { display: flex; align-items: baseline; flex-wrap: wrap; gap: 6px; margin-top: 12px; color: var(--danger); font-size: 13.5px; }
  .error :global(svg) { flex: none; transform: translateY(2px); }
  .link { border: 0; padding: 0; background: none; color: var(--accent); font-size: 13px; font-weight: 500; }
  .foot { display: flex; align-items: center; gap: 12px; margin-top: 14px; font-size: 12px; color: var(--muted); }
  .link.quiet { display: inline-flex; align-items: center; gap: 4px; color: var(--muted); font-size: 12px; font-weight: 400; }
  .link.quiet:hover { color: var(--text-2); }
  .steps { margin: 8px 0 0; padding-left: 20px; display: flex; flex-direction: column; gap: 3px; font-size: 12px; color: var(--muted); }
  .steps li.note { font-style: italic; }
  .steps li.err { color: var(--danger); }
  .ask {
    flex: none;
    display: flex;
    align-items: flex-end;
    gap: 6px;
    width: min(680px, calc(100% - 48px));
    margin: 0 auto;
    padding: 6px 6px 6px 14px;
    border: 1px solid var(--line-strong);
    border-radius: 12px;
    background: var(--field);
  }
  .ask:focus-within { border-color: var(--accent); }
  textarea {
    flex: 1;
    min-width: 0;
    max-height: 120px;
    padding: 5px 0;
    border: 0;
    outline: none;
    resize: none;
    background: transparent;
    font: inherit;
    font-size: 14px;
    line-height: 1.45;
    field-sizing: content;
  }
  .send:not(:disabled) { color: var(--accent); }
  .send:disabled { opacity: 0.35; }
  .model { flex: none; text-align: center; padding: 6px 0 12px; font-size: 11.5px; color: var(--muted); }
  .spinner {
    width: 13px;
    height: 13px;
    flex: none;
    border-radius: 50%;
    border: 2px solid var(--line-strong);
    border-top-color: var(--muted);
    animation: spin 0.8s linear infinite;
  }
  .peek {
    position: fixed;
    z-index: 40;
    width: 300px;
    padding: 10px 12px;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 10px;
    box-shadow: var(--shadow);
    font-size: 12.5px;
    pointer-events: none;
    animation: fade 0.1s ease-out;
  }
  .peek-head { display: flex; gap: 8px; align-items: baseline; }
  .peek-from { flex: 1; min-width: 0; font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .peek-date { flex: none; color: var(--muted); font-size: 11.5px; }
  .peek-subject { margin-top: 2px; color: var(--text-2); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .peek-snip { margin-top: 4px; color: var(--muted); display: -webkit-box; -webkit-line-clamp: 3; line-clamp: 3; -webkit-box-orient: vertical; overflow: hidden; }
  @container (max-width: 520px) {
    .body { padding: 4px 20px 20px; }
  }
</style>
