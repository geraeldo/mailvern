<script lang="ts" generics="T">
  /**
   * The app's dropdown. Use it instead of a native <select>, which draws the
   * system's grey pill and menu and never matches the rest of the app.
   */
  import Icon from "./Icon.svelte";

  type Option = { value: T; label: string; detail?: string };

  let {
    value,
    options,
    onchange,
    plain = false,
    label = undefined,
    prompt = undefined,
  }: {
    value: T;
    options: Option[];
    onchange: (v: T) => void;
    /** No border or fill until hovered: for a field row such as the composer's From. */
    plain?: boolean;
    /** Accessible name, when no visible label points at it. */
    label?: string;
    /** Makes it an add menu: a "+ prompt" button whose pick is handed over, never shown as chosen. */
    prompt?: string;
  } = $props();

  let open = $state(false);
  let active = $state(0);
  /** Where the menu sits on screen. It's fixed, so a scrolling pane around the trigger never clips it. */
  let place = $state({ left: 0, top: "auto", bottom: "auto", minWidth: 0, maxHeight: 280 });
  let root = $state<HTMLDivElement>();
  let trigger = $state<HTMLButtonElement>();
  let listEl = $state<HTMLDivElement>();

  const current = $derived(options.find((o) => o.value === value));

  function show() {
    if (!root) return;
    const r = root.getBoundingClientRect();
    const below = window.innerHeight - r.bottom - 16;
    const above = r.top - 16;
    const up = below < 200 && above > below;
    place = {
      left: r.left,
      top: up ? "auto" : `${r.bottom + 4}px`,
      bottom: up ? `${window.innerHeight - r.top + 4}px` : "auto",
      minWidth: r.width,
      maxHeight: Math.max(120, Math.min(280, up ? above : below)),
    };
    active = Math.max(0, options.findIndex((o) => o.value === value));
    open = true;
    queueMicrotask(() => {
      if (!listEl) return;
      // A trigger on the right of the window, or one with no room to its right, gets a menu
      // whose right edge lines up with it, so the menu stays over the pane it came from.
      const w = listEl.offsetWidth;
      const rightSide = r.left + r.width / 2 > window.innerWidth / 2;
      if (rightSide || r.left + w > window.innerWidth - 8) place.left = Math.max(8, r.right - w);
      listEl.querySelectorAll<HTMLElement>(".opt")[active]?.scrollIntoView({ block: "nearest" });
    });
  }

  function hide(refocus = true) {
    open = false;
    if (refocus) trigger?.focus();
  }

  function pick(o: Option) {
    hide();
    if (prompt || o.value !== value) onchange(o.value);
  }

  function move(to: number) {
    active = Math.max(0, Math.min(options.length - 1, to));
    listEl?.querySelectorAll<HTMLElement>(".opt")[active]?.scrollIntoView({ block: "nearest" });
  }

  function onKey(e: KeyboardEvent) {
    if (!open) {
      if (["ArrowDown", "ArrowUp", "Enter", " "].includes(e.key)) {
        e.preventDefault();
        show();
      }
      return;
    }
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      move(active + (e.key === "ArrowDown" ? 1 : -1));
    } else if (e.key === "Home" || e.key === "End") {
      e.preventDefault();
      move(e.key === "Home" ? 0 : options.length - 1);
    } else if ((e.key === "Enter" || e.key === " ") && options[active]) {
      e.preventDefault();
      pick(options[active]);
    } else if (e.key === "Escape") {
      // Closes the menu only, not the dialog it sits in.
      e.preventDefault();
      e.stopPropagation();
      hide();
    } else if (e.key === "Tab") {
      hide(false);
    }
  }
</script>

<svelte:window
  onmousedown={(e) => { if (open && !root?.contains(e.target as Node)) hide(false); }}
  onresize={() => open && hide(false)}
  onscrollcapture={(e) => { if (open && !listEl?.contains(e.target as Node)) hide(false); }} />

<div class="select" class:plain class:menu={prompt} bind:this={root}>
  <button
    bind:this={trigger}
    type="button"
    class="current"
    class:open
    aria-haspopup="listbox"
    aria-expanded={open}
    aria-label={label}
    onclick={() => (open ? hide() : show())}
    onkeydown={onKey}>
    {#if prompt}
      <Icon name="plus" size={14} /><span class="lbl">{prompt}</span>
    {:else}
      <span class="text">
        <span class="lbl">{current?.label ?? ""}</span>
        {#if current?.detail}<span class="detail">{current.detail}</span>{/if}
      </span>
      <span class="caret"><Icon name="chevronDown" size={13} stroke={2} /></span>
    {/if}
  </button>
  {#if open}
    <div
      class="pop scroll"
      role="listbox"
      bind:this={listEl}
      style:left="{place.left}px"
      style:top={place.top}
      style:bottom={place.bottom}
      style:min-width="max({place.minWidth}px, 180px)"
      style:max-height="{place.maxHeight}px">
      {#each options as o, i (o.value)}
        <button
          type="button"
          tabindex="-1"
          role="option"
          aria-selected={o.value === value}
          class="opt"
          class:active={i === active}
          onmouseenter={() => (active = i)}
          onclick={() => pick(o)}>
          <span class="text">
            <span class="lbl">{o.label}</span>
            {#if o.detail}<span class="detail">{o.detail}</span>{/if}
          </span>
          <span class="tick">{#if !prompt && o.value === value}<Icon name="check" size={13} stroke={2.2} />{/if}</span>
        </button>
      {/each}
    </div>
  {/if}
</div>

<style>
  .select { position: relative; min-width: 0; }
  .current {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    max-width: 100%;
    height: 30px;
    padding: 0 8px 0 10px;
    border: 1px solid var(--line-strong);
    border-radius: 8px;
    background: var(--field);
    font-size: 13px;
    text-align: left;
  }
  .current:hover, .current.open { background: var(--hover); }
  .current:focus-visible { outline: 2px solid var(--accent); outline-offset: 1px; }
  /* Pulled left so the text, not the hover fill, lines up with the field. */
  .plain { margin-left: -8px; }
  .plain .current {
    height: 30px;
    padding: 0 6px 0 8px;
    border-color: transparent;
    background: transparent;
    font-size: 14px;
  }
  .plain .current:hover, .plain .current.open { background: var(--hover); }
  .menu .current {
    gap: 6px;
    height: 26px;
    padding: 0 8px;
    border-color: transparent;
    background: transparent;
    font-size: 12.5px;
    color: var(--text-2);
  }
  .menu .current:hover, .menu .current.open { background: var(--hover); }
  .text { display: flex; align-items: baseline; gap: 6px; min-width: 0; }
  .lbl { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .detail { flex-shrink: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--muted); font-size: 0.93em; }
  .caret { display: grid; flex: none; color: var(--muted); }
  .pop {
    position: fixed;
    z-index: 100;
    max-width: min(460px, calc(100vw - 32px));
    padding: 4px;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 10px;
    box-shadow: var(--shadow);
    animation: rise 0.12s ease-out;
  }
  .opt {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    min-height: 30px;
    padding: 5px 8px;
    border: 0;
    border-radius: 7px;
    background: transparent;
    font-size: 13px;
    text-align: left;
    white-space: nowrap;
  }
  .opt.active { background: var(--hover); }
  .opt .text { flex: 1; }
  .tick { display: grid; flex: none; width: 13px; color: var(--accent); }
</style>
