<script lang="ts">
  import Icon from "./Icon.svelte";
  import { listModels, price, type ModelInfo } from "../ai/config.svelte";

  let {
    value,
    onpick,
    needsTools = false,
  }: { value: ModelInfo | null; onpick: (m: ModelInfo) => void; needsTools?: boolean } = $props();

  let open = $state(false);
  let query = $state("");
  let models = $state<ModelInfo[]>([]);
  let error = $state("");
  let loading = $state(false);
  let active = $state(0);
  let input = $state<HTMLInputElement>();
  let listEl = $state<HTMLDivElement>();
  let root = $state<HTMLDivElement>();
  let popEl = $state<HTMLDivElement>();
  let trigger = $state<HTMLButtonElement>();
  let place = $state({ left: 0, top: "auto", bottom: "auto", listMax: 300 });

  const shown = $derived.by(() => {
    const words = query.toLowerCase().split(/\s+/).filter(Boolean);
    return models
      .filter((m) => !needsTools || m.tools)
      .filter((m) => words.every((w) => `${m.name} ${m.id}`.toLowerCase().includes(w)));
  });

  async function toggle() {
    open = !open;
    if (!open) return;
    if (root) {
      const r = root.getBoundingClientRect();
      const popWidth = Math.min(380, window.innerWidth - 24);
      const below = window.innerHeight - r.bottom - 16;
      const above = r.top - 16;
      const up = below < 280 && above > below;
      const left = Math.max(12, Math.min(r.right - popWidth, window.innerWidth - popWidth - 12));
      place = {
        left,
        top: up ? "auto" : `${r.bottom + 6}px`,
        bottom: up ? `${window.innerHeight - r.top + 6}px` : "auto",
        listMax: Math.max(120, Math.min(300, (up ? above : below) - 56)),
      };
    }
    query = "";
    active = 0;
    queueMicrotask(() => input?.focus());
    if (models.length) return;
    loading = true;
    error = "";
    try {
      models = await listModels();
    } catch (e) {
      error = `Couldn't load models: ${e}`;
    } finally {
      loading = false;
    }
  }

  function close(refocus = false) {
    open = false;
    if (refocus) trigger?.focus();
  }

  function pick(m: ModelInfo) {
    onpick(m);
    close(true);
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      if (!shown.length) return;
      active = Math.max(0, Math.min(shown.length - 1, active + (e.key === "ArrowDown" ? 1 : -1)));
      listEl?.querySelectorAll<HTMLElement>(".opt")[active]?.scrollIntoView({ block: "nearest" });
    } else if (e.key === "Enter" && shown[active]) {
      e.preventDefault();
      pick(shown[active]);
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      close(true);
    }
  }
</script>

<svelte:window
  onmousedown={(e) => {
    const t = e.target as Node;
    if (open && !root?.contains(t) && !popEl?.contains(t)) close();
  }}
  onresize={() => open && close()}
  onscrollcapture={(e) => { if (open && !popEl?.contains(e.target as Node)) close(); }} />

<div class="picker" bind:this={root}>
  <button type="button" class="current" class:open bind:this={trigger} aria-expanded={open} onclick={toggle}>
    <span class="name">{value?.name ?? "Choose a model"}</span>
    <span class="caret"><Icon name="chevronDown" size={13} stroke={2} /></span>
  </button>
  {#if open}
    <div
      class="pop"
      bind:this={popEl}
      style:left="{place.left}px"
      style:top={place.top}
      style:bottom={place.bottom}>
      <input
        bind:this={input}
        bind:value={query}
        oninput={() => (active = 0)}
        onkeydown={onKey}
        class="search"
        placeholder="Search models"
        spellcheck="false" />
      <div class="list scroll" bind:this={listEl} style:max-height="{place.listMax}px">
        {#if loading}
          <div class="note"><span class="spinner"></span></div>
        {:else if error}
          <div class="note">{error}</div>
        {:else}
          {#each shown as m, i (m.id)}
            <button type="button" class="opt" class:on={m.id === value?.id} class:active={i === active} onmouseenter={() => (active = i)} onclick={() => pick(m)}>
              <span class="row1">
                <span class="mname">{m.name}</span>
                {#if m.vision}<span class="tag">Images</span>{/if}
                {#if m.id === value?.id}<Icon name="check" size={13} stroke={2.2} />{/if}
              </span>
              <span class="row2">{m.id} · {price(m)}</span>
            </button>
          {:else}
            <div class="note">No models match</div>
          {/each}
        {/if}
      </div>
    </div>
  {/if}
</div>

<style>
  /* Never shrinks under the label next to it; the model name truncates instead. */
  .picker { position: relative; flex: none; max-width: min(260px, 48%); }
  .current {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    max-width: 100%;
    height: 30px;
    padding: 0 8px 0 10px;
    border: 1px solid var(--line-strong);
    border-radius: 8px;
    background: var(--field);
    font-size: 13px;
  }
  .current:hover, .current.open { background: var(--hover); }
  .name { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .caret { display: grid; flex: none; color: var(--muted); }
  .pop {
    position: fixed;
    z-index: 100;
    width: min(380px, calc(100vw - 24px));
    padding: 6px;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 10px;
    box-shadow: var(--shadow);
    animation: rise 0.12s ease-out;
  }
  .search {
    width: 100%;
    height: 32px;
    padding: 0 10px;
    border: 1px solid var(--line-strong);
    border-radius: 7px;
    background: var(--field);
    outline: none;
    font-size: 13px;
  }
  .search:focus { border-color: var(--accent); box-shadow: 0 0 0 3px var(--accent-soft); }
  .list { margin-top: 6px; }
  .opt {
    display: flex;
    flex-direction: column;
    align-items: stretch;
    gap: 1px;
    width: 100%;
    max-width: 100%;
    overflow: hidden;
    padding: 6px 8px;
    border: 0;
    border-radius: 7px;
    background: transparent;
    text-align: left;
  }
  .opt.active { background: var(--hover); }
  .opt.on { background: var(--selected); }
  .row1 { display: flex; align-items: center; gap: 6px; width: 100%; min-width: 0; font-size: 13px; color: var(--text); }
  .row1 :global(svg) { color: var(--accent); margin-left: auto; flex: none; }
  .mname { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .tag { flex: none; padding: 0 5px; border-radius: 4px; background: var(--hover); color: var(--muted); font-size: 10.5px; font-weight: 600; line-height: 16px; }
  .row2 { display: block; width: 100%; font-size: 11.5px; color: var(--muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .note { display: grid; place-items: center; padding: 18px; color: var(--muted); font-size: 12.5px; }
  .spinner {
    width: 16px;
    height: 16px;
    border-radius: 50%;
    border: 2px solid var(--line-strong);
    border-top-color: var(--muted);
    animation: spin 0.8s linear infinite;
  }
</style>
