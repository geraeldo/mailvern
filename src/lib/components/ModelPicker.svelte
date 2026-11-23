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

  const shown = $derived.by(() => {
    const words = query.toLowerCase().split(/\s+/).filter(Boolean);
    return models
      .filter((m) => !needsTools || m.tools)
      .filter((m) => words.every((w) => `${m.name} ${m.id}`.toLowerCase().includes(w)));
  });

  async function toggle() {
    open = !open;
    if (!open) return;
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

  function pick(m: ModelInfo) {
    onpick(m);
    open = false;
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      active = Math.max(0, Math.min(shown.length - 1, active + (e.key === "ArrowDown" ? 1 : -1)));
      listEl?.querySelectorAll<HTMLElement>(".opt")[active]?.scrollIntoView({ block: "nearest" });
    } else if (e.key === "Enter" && shown[active]) {
      e.preventDefault();
      pick(shown[active]);
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      open = false;
    }
  }
</script>

<svelte:window onmousedown={(e) => { if (open && !(e.target as Element).closest?.(".picker")) open = false; }} />

<div class="picker">
  <button class="current" onclick={toggle}>
    <span class="name">{value?.name ?? "Choose a model"}</span>
    <span class="caret"><Icon name="chevronDown" size={13} stroke={2} /></span>
  </button>
  {#if open}
    <div class="pop">
      <input
        bind:this={input}
        bind:value={query}
        oninput={() => (active = 0)}
        onkeydown={onKey}
        class="search"
        placeholder="Search models"
        spellcheck="false" />
      <div class="list scroll" bind:this={listEl}>
        {#if loading}
          <div class="note"><span class="spinner"></span></div>
        {:else if error}
          <div class="note">{error}</div>
        {:else}
          {#each shown as m, i (m.id)}
            <button class="opt" class:on={m.id === value?.id} class:active={i === active} onmouseenter={() => (active = i)} onclick={() => pick(m)}>
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
  .picker { position: relative; }
  .current {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    max-width: 260px;
    height: 30px;
    padding: 0 8px 0 10px;
    border: 1px solid var(--line-strong);
    border-radius: 8px;
    background: var(--field);
    font-size: 13px;
  }
  .current:hover { background: var(--hover); }
  .name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .caret { display: grid; color: var(--muted); }
  .pop {
    position: absolute;
    right: 0;
    top: calc(100% + 6px);
    z-index: 30;
    width: 380px;
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
  .search:focus { border-color: var(--accent); }
  .list { max-height: 320px; margin-top: 6px; }
  .opt {
    display: flex;
    flex-direction: column;
    gap: 1px;
    width: 100%;
    padding: 6px 8px;
    border: 0;
    border-radius: 7px;
    background: transparent;
    text-align: left;
  }
  .opt.active { background: var(--hover); }
  .opt.on { background: var(--selected); }
  .row1 { display: flex; align-items: center; gap: 6px; font-size: 13px; color: var(--text); }
  .row1 :global(svg) { color: var(--accent); margin-left: auto; flex: none; }
  .mname { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .tag { flex: none; padding: 0 5px; border-radius: 4px; background: var(--hover); color: var(--muted); font-size: 10.5px; font-weight: 600; line-height: 16px; }
  .row2 { font-size: 11.5px; color: var(--muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
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
