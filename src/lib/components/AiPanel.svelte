<script lang="ts">
  import Icon from "./Icon.svelte";
  import { ui, openMessage, type AiSearch } from "../state.svelte";
  import { stopAiSearch } from "../ai/search.svelte";
  import { escapeHtml } from "../html";
  import { displayName } from "../format";

  let { s, onclose }: { s: AiSearch; onclose: () => void } = $props();
  let showAll = $state(false);

  const steps = $derived(s.running || showAll ? s.steps : s.steps.slice(-2));

  /** The answer as safe HTML: bold, bullets, line breaks, and [#id] citations as links. */
  const html = $derived.by(() => {
    const label = (id: number) => {
      const m = ui.list.find((r) => r.id === id);
      return m ? displayName(m.fromName, m.fromAddr) || m.subject || "message" : "message";
    };
    return escapeHtml(s.answer)
      .replace(/\*\*(.+?)\*\*/g, "<b>$1</b>")
      .replace(/\[#(\d+)\]/g, (_, id) => `<button class="cite" data-id="${id}">${escapeHtml(label(Number(id)))}</button>`)
      .split("\n")
      .map((l) => (/^\s*[-•*]\s+/.test(l) ? `<span class="li">${l.replace(/^\s*[-•*]\s+/, "")}</span>` : l))
      .join("<br>")
      .replace(/<\/span><br>/g, "</span>");
  });

  function onClick(e: MouseEvent) {
    const cite = (e.target as Element).closest?.<HTMLElement>(".cite");
    if (cite) openMessage(Number(cite.dataset.id));
  }

  const cost = (n: number) => (n <= 0 ? "" : n < 0.01 ? "<$0.01" : `$${n.toFixed(2)}`);
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="ai scroll" onclick={onClick}>
  <div class="q">
    <span class="mark"><Icon name="sparkle" size={14} /></span>
    <span class="question">{s.question}</span>
    {#if s.running}
      <button class="icon-btn small" title="Stop" onclick={stopAiSearch}><Icon name="stop" size={13} /></button>
    {:else}
      <button class="icon-btn small" title="Back to search results" onclick={onclose}><Icon name="x" size={13} /></button>
    {/if}
  </div>

  {#if s.steps.length}
    <ul class="steps">
      {#if !s.running && !showAll && s.steps.length > 2}
        <li><button class="more" onclick={() => (showAll = true)}>{s.steps.length - 2} earlier steps</button></li>
      {/if}
      {#each steps as st, i (i)}
        <li class:note={st.kind === "note"} class:err={st.error}>
          {#if !st.done}<span class="spinner"></span>{:else if st.kind !== "note"}<span class="dot"></span>{/if}
          <span>{st.text}</span>
        </li>
      {/each}
    </ul>
  {/if}

  {#if s.running && !s.steps.some((x) => !x.done)}
    <div class="thinking"><span class="spinner"></span> Thinking…</div>
  {/if}

  {#if s.answer && (!s.running || !s.steps.length || s.steps.every((x) => x.done))}
    <div class="answer">{@html html}</div>
  {/if}
  {#if s.error}
    <div class="error"><Icon name="alert" size={13} /> {s.error}</div>
  {/if}
  {#if !s.running}
    <div class="meta">
      {s.ids.length} message{s.ids.length === 1 ? "" : "s"} · {s.model}{cost(s.cost) ? ` · ${cost(s.cost)}` : ""}
    </div>
  {/if}
</div>

<style>
  .ai {
    flex: none;
    max-height: 46%;
    margin: 0 12px 10px;
    padding: 10px 12px;
    border-radius: 10px;
    background: var(--side);
    border: 1px solid var(--line);
    font-size: 13px;
    -webkit-user-select: text;
    user-select: text;
  }
  .q { display: flex; align-items: flex-start; gap: 8px; }
  .mark { color: var(--accent); display: grid; padding-top: 2px; }
  .question { flex: 1; font-weight: 600; line-height: 1.4; }
  .icon-btn.small { width: 22px; height: 22px; margin: -2px -4px 0 0; flex: none; }
  .steps { list-style: none; margin: 8px 0 0; padding: 0 0 0 22px; display: flex; flex-direction: column; gap: 3px; color: var(--muted); font-size: 12px; }
  .steps li { display: flex; align-items: baseline; gap: 7px; line-height: 1.4; }
  .steps li > span:last-child { min-width: 0; overflow-wrap: anywhere; }
  .steps li.note { font-style: italic; }
  .steps li.err { color: var(--danger); }
  .dot { width: 4px; height: 4px; border-radius: 50%; background: var(--line-strong); flex: none; transform: translateY(-2px); }
  .more { border: 0; padding: 0; background: none; color: var(--muted); font-size: 12px; text-decoration: underline; text-underline-offset: 2px; }
  .thinking { display: flex; align-items: center; gap: 7px; margin: 6px 0 0 22px; color: var(--muted); font-size: 12px; }
  .spinner {
    width: 10px;
    height: 10px;
    flex: none;
    border-radius: 50%;
    border: 1.5px solid var(--line-strong);
    border-top-color: var(--muted);
    animation: spin 0.8s linear infinite;
    align-self: center;
  }
  .answer { margin: 8px 0 0 22px; line-height: 1.5; color: var(--text); overflow-wrap: anywhere; }
  .answer :global(.li) { display: block; padding-left: 12px; position: relative; }
  .answer :global(.li)::before { content: "•"; position: absolute; left: 0; color: var(--muted); }
  .answer :global(.cite) {
    display: inline;
    padding: 0 5px;
    margin: 0 1px;
    border: 0;
    border-radius: 5px;
    background: var(--accent-soft);
    color: var(--accent);
    font-size: 12px;
    font-weight: 500;
    cursor: pointer;
  }
  .error { display: flex; align-items: flex-start; gap: 6px; margin: 8px 0 0 22px; color: var(--danger); font-size: 12.5px; }
  .error :global(svg) { flex: none; margin-top: 2px; }
  .meta { margin: 8px 0 0 22px; color: var(--muted); font-size: 11.5px; }
</style>
