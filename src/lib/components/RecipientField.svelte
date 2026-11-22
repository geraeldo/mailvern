<script lang="ts">
  import { api, type Addr } from "../api";
  import { formatAddr } from "../format";

  let { label, value = $bindable([]), autofocus = false }: { label: string; value: string[]; autofocus?: boolean } = $props();

  let input = $state("");
  let inputEl: HTMLInputElement;
  let suggestions = $state<Addr[]>([]);
  let hi = $state(0);

  const valid = (s: string) => /^[^\s@<>]+@[^\s@<>]+\.[^\s@<>]+$/.test(s.match(/<([^>]+)>/)?.[1] ?? s);

  $effect(() => {
    if (autofocus) inputEl?.focus();
  });

  function commit(raw: string) {
    const parts = raw.split(/[,;\n]+/).map((s) => s.trim()).filter(Boolean);
    if (parts.length) value = [...value, ...parts.filter((p) => !value.includes(p))];
    input = "";
    suggestions = [];
  }

  let seq = 0;
  async function suggest() {
    const q = input.trim();
    const my = ++seq;
    if (q.length < 1) {
      suggestions = [];
      return;
    }
    const res = await api.contacts(q);
    if (my === seq) {
      suggestions = res.filter((a) => !value.some((v) => v.includes(a.email)));
      hi = 0;
    }
  }

  function pick(a: Addr) {
    commit(formatAddr(a));
    inputEl.focus();
  }

  function onKey(e: KeyboardEvent) {
    if (suggestions.length && (e.key === "ArrowDown" || e.key === "ArrowUp")) {
      e.preventDefault();
      hi = (hi + (e.key === "ArrowDown" ? 1 : -1) + suggestions.length) % suggestions.length;
    } else if ((e.key === "Enter" || e.key === "Tab") && suggestions.length) {
      e.preventDefault();
      pick(suggestions[hi]);
    } else if ((e.key === "Enter" || e.key === "," || e.key === ";") && input.trim()) {
      e.preventDefault();
      commit(input);
    } else if (e.key === "Backspace" && !input && value.length) {
      value = value.slice(0, -1);
    } else if (e.key === "Escape" && suggestions.length) {
      e.stopPropagation();
      suggestions = [];
    }
  }

  const short = (v: string) => v.match(/^\s*"?([^"<]+?)"?\s*</)?.[1] ?? v;

  export function flush() {
    if (input.trim()) commit(input);
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="row" onclick={() => inputEl.focus()}>
  <span class="label">{label}</span>
  <div class="chips">
    {#each value as v, i (v)}
      <span class="chip" class:bad={!valid(v)} title={v}>
        {short(v)}
        <button type="button" aria-label="Remove" onclick={() => (value = value.filter((_, j) => j !== i))}>×</button>
      </span>
    {/each}
    <input
      bind:this={inputEl}
      bind:value={input}
      oninput={suggest}
      onkeydown={onKey}
      onblur={() => setTimeout(() => { if (input.trim()) commit(input); suggestions = []; }, 120)}
      spellcheck="false"
      autocomplete="off" />
  </div>
  {#if suggestions.length}
    <ul class="pop">
      {#each suggestions as s, i (s.email)}
        <li>
          <button type="button" class:hi={i === hi} onmousedown={(e) => { e.preventDefault(); pick(s); }}>
            <span class="n">{s.name ?? s.email}</span>
            {#if s.name}<span class="e">{s.email}</span>{/if}
          </button>
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .row {
    position: relative;
    display: flex;
    align-items: center;
    gap: 10px;
    min-height: 42px;
    padding: 5px 0;
    border-bottom: 1px solid var(--line);
  }
  .label { width: 52px; flex: none; color: var(--muted); font-size: 13px; }
  .chips { flex: 1; display: flex; flex-wrap: wrap; align-items: center; gap: 5px; min-width: 0; }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    height: 26px;
    padding: 0 4px 0 10px;
    border-radius: 13px;
    background: var(--accent-soft);
    color: var(--text);
    font-size: 13px;
    max-width: 260px;
    white-space: nowrap;
    overflow: hidden;
  }
  .chip.bad { background: rgba(214, 64, 46, 0.12); color: var(--danger); }
  .chip button { border: 0; background: none; color: var(--muted); width: 20px; height: 20px; border-radius: 50%; padding: 0; font-size: 15px; line-height: 1; }
  .chip button:hover { background: var(--hover); color: var(--text); }
  input { flex: 1; min-width: 120px; border: 0; outline: none; background: transparent; height: 28px; font-size: 14px; }
  .pop {
    position: absolute;
    top: calc(100% + 4px);
    left: 56px;
    z-index: 10;
    margin: 0;
    padding: 5px;
    list-style: none;
    min-width: 320px;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 10px;
    box-shadow: var(--shadow);
  }
  .pop button {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    width: 100%;
    padding: 6px 10px;
    border: 0;
    border-radius: 7px;
    background: transparent;
    text-align: left;
  }
  .pop button.hi { background: var(--selected); }
  .n { font-size: 13.5px; font-weight: 500; }
  .e { font-size: 12px; color: var(--muted); }
</style>
