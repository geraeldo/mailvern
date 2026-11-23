<script lang="ts">
  import Icon from "./Icon.svelte";
  import { MODES, rephrase, type RephraseContext } from "../ai/rephrase";
  import { writeReady, ai } from "../ai/config.svelte";
  import { htmlToPlain } from "../ai/text";
  import { toast } from "../state.svelte";

  let {
    editor,
    context,
    onapplied,
  }: { editor: HTMLDivElement | undefined; context: () => RephraseContext; onapplied: () => void } = $props();

  type Phase = "closed" | "menu" | "working" | "done" | "error";
  let phase = $state<Phase>("closed");
  let target: Range | null = null;
  let kind = $state<"selection" | "message">("message");
  let custom = $state("");
  let label = $state("");
  let instruction = "";
  let result = $state("");
  let error = $state("");
  let controller: AbortController | null = null;
  let customEl = $state<HTMLInputElement>();

  /** Where the user's own writing ends: before the signature or the quoted message. */
  function messageRange(el: HTMLDivElement): Range {
    const r = document.createRange();
    const kids = Array.from(el.childNodes);
    let end = kids.findIndex(
      (n) => n instanceof Element && (n.matches('[data-signature], .mv-quote-head, blockquote[type="cite"]') || !!n.querySelector('.mv-quote-head, blockquote[type="cite"]')),
    );
    if (end < 0) end = kids.length;
    if (end === 0) {
      r.setStart(el, 0);
      r.collapse(true);
      return r;
    }
    // Start and end inside the first and last blocks, not between them: replacing a range that
    // ends right before the signature makes the browser drop the signature's leading line break.
    const first = kids[0];
    const last = kids[end - 1];
    r.setStart(first, 0);
    r.setEnd(last, last.nodeType === Node.TEXT_NODE ? (last.textContent ?? "").length : last.childNodes.length);
    return r;
  }

  function htmlOf(r: Range): string {
    const div = document.createElement("div");
    div.appendChild(r.cloneContents());
    return div.innerHTML;
  }

  function open() {
    if (phase !== "closed") {
      close();
      return;
    }
    if (!ai.hasKey) {
      toast("Add an OpenRouter key in Settings › Models");
      return;
    }
    if (!writeReady()) {
      toast("Choose a writing model in Settings › Models");
      return;
    }
    if (!editor) return;
    const sel = window.getSelection();
    if (sel && sel.rangeCount && !sel.isCollapsed && editor.contains(sel.anchorNode) && editor.contains(sel.focusNode) && sel.toString().trim()) {
      target = sel.getRangeAt(0).cloneRange();
      kind = "selection";
    } else {
      target = messageRange(editor);
      kind = "message";
    }
    if (!htmlToPlain(htmlOf(target)).trim()) {
      toast("Nothing to rephrase yet");
      target = null;
      return;
    }
    custom = "";
    phase = "menu";
  }

  async function run(text: string, instr: string) {
    if (!target || !editor) return;
    label = text;
    instruction = instr;
    result = "";
    error = "";
    phase = "working";
    controller?.abort();
    const my = new AbortController();
    controller = my;
    const ctx = context();
    if (kind === "selection") ctx.whole = htmlToPlain(htmlOf(messageRange(editor)));
    try {
      const html = await rephrase(htmlOf(target), instr, ctx, (h) => my === controller && (result = h), my.signal);
      if (my !== controller) return;
      result = html;
      phase = result.trim() ? "done" : "error";
      if (!result.trim()) error = "No response. Try again or pick another model.";
    } catch (e) {
      if (my !== controller) return;
      if (my.signal.aborted) {
        phase = result.trim() ? "done" : "menu";
        return;
      }
      error = e instanceof Error ? e.message : String(e);
      phase = "error";
    }
  }

  function stop() {
    controller?.abort();
  }

  function replace() {
    if (!target || !editor || !result) return;
    editor.focus();
    const sel = window.getSelection();
    sel?.removeAllRanges();
    sel?.addRange(target);
    // Through the editing commands, so ⌘Z takes it back.
    document.execCommand("insertHTML", false, result);
    onapplied();
    close();
  }

  function close() {
    controller?.abort();
    controller = null;
    phase = "closed";
    target = null;
  }

  /** Runs before the composer's own keys: while rephrasing, ⌘↩ means Replace, never Send. */
  function onKey(e: KeyboardEvent) {
    if (phase === "closed") return;
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      close();
    } else if (e.key === "Enter" && e.metaKey) {
      e.preventDefault();
      e.stopPropagation();
      if (phase === "done") replace();
    }
  }

  $effect(() => {
    if (phase === "menu") queueMicrotask(() => customEl?.focus());
  });

  const keep = (e: MouseEvent) => e.preventDefault();
</script>

<svelte:window
  onkeydowncapture={onKey}
  onmousedown={(e) => { if (phase === "menu" && !(e.target as Element).closest?.(".rephrase")) close(); }} />

<span class="rephrase">
  <button type="button" class="icon-btn" class:on={phase !== "closed"} title="Rephrase with AI" onmousedown={keep} onclick={open}>
    <Icon name="sparkle" size={16} />
  </button>

  {#if phase === "menu"}
    <div class="pop menu">
      <div class="head">Rephrase {kind === "selection" ? "selection" : "message"}</div>
      {#each MODES as m (m.id)}
        <button type="button" class="opt" onmousedown={keep} onclick={() => run(m.label, m.instruction)}>{m.label}</button>
      {/each}
      <input
        bind:this={customEl}
        bind:value={custom}
        class="custom"
        placeholder="Or describe a change…"
        spellcheck="true"
        onkeydown={(e) => {
          if (e.key === "Enter" && custom.trim()) {
            e.preventDefault();
            run(custom.trim(), custom.trim());
          }
        }} />
    </div>
  {:else if phase !== "closed"}
    <div class="pop preview">
      <div class="head">
        <Icon name="sparkle" size={13} />
        <span class="what">{label}</span>
        <span class="who">{ai.write?.name ?? ""}</span>
      </div>
      {#if phase === "error"}
        <div class="error"><Icon name="alert" size={13} /> {error}</div>
      {:else}
        <div class="result scroll">
          {#if result}{@html result}{:else}<span class="wait"><span class="spinner"></span> Writing…</span>{/if}
        </div>
      {/if}
      <div class="actions">
        {#if phase === "working"}
          <button type="button" class="btn" onclick={stop}><Icon name="stop" size={12} /> Stop</button>
        {:else}
          <button type="button" class="btn ghost" onclick={() => (phase = "menu")}>Back</button>
          <button type="button" class="btn ghost" onclick={() => run(label, instruction)}><Icon name="refresh" size={13} /> Try again</button>
          <span class="grow"></span>
          <button type="button" class="btn ghost" onclick={close}>Discard</button>
          {#if phase === "done"}
            <button type="button" class="btn primary" title="Replace  ⌘↩" onclick={replace}>Replace</button>
          {/if}
        {/if}
      </div>
    </div>
  {/if}
</span>

<style>
  .rephrase { position: relative; display: inline-flex; }
  .pop {
    position: absolute;
    right: -4px;
    bottom: calc(100% + 10px);
    z-index: 20;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 10px;
    box-shadow: var(--shadow);
    animation: rise 0.12s ease-out;
  }
  .menu { width: 250px; padding: 6px; display: flex; flex-direction: column; }
  .head { display: flex; align-items: center; gap: 6px; font-size: 11.5px; font-weight: 600; color: var(--muted); padding: 4px 8px 6px; }
  .head :global(svg) { color: var(--accent); flex: none; }
  .opt {
    text-align: left;
    padding: 6px 8px;
    border: 0;
    border-radius: 7px;
    background: transparent;
    font-size: 13.5px;
  }
  .opt:hover { background: var(--hover); }
  .custom {
    margin-top: 6px;
    height: 32px;
    padding: 0 9px;
    border: 1px solid var(--line-strong);
    border-radius: 7px;
    background: var(--field);
    outline: none;
    font-size: 13px;
  }
  .custom:focus { border-color: var(--accent); }
  .preview { width: min(560px, calc(100vw - 120px)); padding: 10px 12px 10px; }
  .preview .head { padding: 0 0 8px; }
  .what { color: var(--text-2); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .who { margin-left: auto; font-weight: 500; white-space: nowrap; }
  .result {
    max-height: 280px;
    padding: 10px 12px;
    border-radius: 8px;
    background: var(--side);
    font-size: 14px;
    line-height: 1.6;
    overflow-wrap: anywhere;
    -webkit-user-select: text;
    user-select: text;
  }
  .result :global(blockquote) { margin: 0 0 0 2px; padding-left: 12px; border-left: 2px solid var(--line-strong); color: var(--text-2); }
  .result :global(ul), .result :global(ol) { padding-left: 24px; margin: 4px 0; }
  .result :global(a) { color: var(--accent); }
  .result :global(img) { max-width: 120px; height: auto; vertical-align: middle; }
  .wait { display: inline-flex; align-items: center; gap: 8px; color: var(--muted); font-size: 13px; }
  .spinner {
    width: 12px;
    height: 12px;
    border-radius: 50%;
    border: 2px solid var(--line-strong);
    border-top-color: var(--muted);
    animation: spin 0.8s linear infinite;
  }
  .error { display: flex; align-items: flex-start; gap: 6px; color: var(--danger); font-size: 13px; padding: 4px 0 6px; }
  .error :global(svg) { flex: none; margin-top: 2px; }
  .actions { display: flex; align-items: center; gap: 4px; margin-top: 10px; }
  .actions .btn { height: 30px; padding: 0 11px; }
  .grow { flex: 1; }
</style>
