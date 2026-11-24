<script lang="ts">
  import type { Snippet } from "svelte";
  import { api, type Addr } from "../api";
  import { formatAddr } from "../format";

  let {
    label,
    value = $bindable([]),
    autofocus = false,
    trailing,
  }: {
    label: string;
    value: string[];
    autofocus?: boolean;
    trailing?: Snippet;
  } = $props();

  let input = $state("");
  let inputEl: HTMLInputElement;
  let rowEl: HTMLDivElement;
  let suggestions = $state<Addr[]>([]);
  let hi = $state(0);
  let popPos = $state({ left: 0, top: "auto", bottom: "auto", width: 320 });

  const valid = (s: string) => /^[^\s@<>]+@[^\s@<>]+\.[^\s@<>]+$/.test(s.match(/<([^>]+)>/)?.[1] ?? s);

  $effect(() => {
    if (autofocus) inputEl?.focus();
  });

  function positionPop() {
    if (!rowEl) return;
    const r = rowEl.getBoundingClientRect();
    const below = window.innerHeight - r.bottom - 12;
    const above = r.top - 12;
    const up = below < 180 && above > below;
    const labelOffset = label ? 62 : 0;
    const width = Math.min(360, Math.max(260, r.width - labelOffset), window.innerWidth - 24);
    const left = Math.max(12, Math.min(r.left + labelOffset, window.innerWidth - width - 12));
    popPos = {
      left,
      top: up ? "auto" : `${r.bottom + 4}px`,
      bottom: up ? `${window.innerHeight - r.top + 4}px` : "auto",
      width,
    };
  }

  function commit(raw: string): boolean {
    const seen = new Set(value);
    const added: string[] = [];
    for (const p of raw.split(/[,;\n]+/).map((s) => s.trim()).filter(Boolean)) {
      if (!seen.has(p)) {
        seen.add(p);
        added.push(p);
      }
    }
    if (added.length) value = [...value, ...added];
    input = "";
    suggestions = [];
    return added.length > 0;
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
      if (suggestions.length) positionPop();
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
      e.preventDefault();
      e.stopPropagation();
      suggestions = [];
    }
  }

  const short = (v: string) => v.match(/^\s*"?([^"<]+?)"?\s*</)?.[1] ?? v;

  export function flush(): boolean {
    return input.trim() ? commit(input) : false;
  }

  export function invalidAddresses(): string[] {
    return value.filter((v) => !valid(v));
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="row" bind:this={rowEl} onclick={() => inputEl.focus()}>
  <span class="label">{label}</span>
  <div class="chips">
    {#each value as v, i (v)}
      <span class="chip" class:bad={!valid(v)} title={valid(v) ? v : `Invalid address: ${v}`}>
        <span class="chip-text">{short(v)}</span>
        <button type="button" aria-label="Remove" onclick={(e) => { e.stopPropagation(); value = value.filter((_, j) => j !== i); }}>×</button>
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
  {#if trailing}
    <div class="trailing" onclick={(e) => e.stopPropagation()}>{@render trailing()}</div>
  {/if}
  {#if suggestions.length}
    <ul
      class="pop scroll"
      style:left="{popPos.left}px"
      style:top={popPos.top}
      style:bottom={popPos.bottom}
      style:width="{popPos.width}px">
      {#each suggestions as s, i (s.email)}
        <li>
          <button type="button" class:hi={i === hi} onmouseenter={() => (hi = i)} onmousedown={(e) => { e.preventDefault(); pick(s); }}>
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
  .trailing { flex: none; align-self: flex-start; padding-top: 3px; }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    height: 26px;
    padding: 0 4px 0 10px;
    border-radius: 13px;
    background: var(--accent-soft);
    color: var(--text);
    font-size: 13px;
    max-width: min(260px, 100%);
    min-width: 0;
    overflow: hidden;
  }
  .chip-text {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .chip.bad { background: var(--danger-soft); color: var(--danger); }
  .chip button {
    flex: none;
    display: grid;
    place-items: center;
    border: 0;
    background: none;
    color: var(--muted);
    width: 20px;
    height: 20px;
    border-radius: 50%;
    padding: 0;
    font-size: 15px;
    line-height: 1;
  }
  .chip button:hover { background: var(--hover); color: var(--text); }
  .chip.bad button { color: var(--danger); }
  input { flex: 1; min-width: 100px; border: 0; outline: none; background: transparent; height: 28px; font-size: 14px; }
  .pop {
    position: fixed;
    z-index: 100;
    margin: 0;
    padding: 5px;
    list-style: none;
    max-height: 240px;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 10px;
    box-shadow: var(--shadow);
    animation: rise 0.12s ease-out;
  }
  .pop button {
    display: flex;
    flex-direction: column;
    align-items: stretch;
    width: 100%;
    max-width: 100%;
    overflow: hidden;
    padding: 6px 10px;
    border: 0;
    border-radius: 7px;
    background: transparent;
    text-align: left;
  }
  .pop button.hi { background: var(--selected); }
  .n { display: block; width: 100%; font-size: 13.5px; font-weight: 500; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .e { display: block; width: 100%; font-size: 12px; color: var(--muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
</style>
