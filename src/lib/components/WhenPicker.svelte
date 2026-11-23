<script lang="ts">
  import Modal from "./Modal.svelte";
  import { type Preset, toLocalInput, whenLabel } from "../when";

  let { title, presets, action, onpick, onclose }: {
    title: string;
    presets: Preset[];
    /** Button text for a custom time, e.g. "Snooze" or "Schedule". */
    action: string;
    onpick: (at: Date) => void;
    onclose: () => void;
  } = $props();

  const tomorrow = new Date();
  tomorrow.setDate(tomorrow.getDate() + 1);
  tomorrow.setHours(9, 0, 0, 0);
  let custom = $state(toLocalInput(tomorrow));
  const customDate = $derived(custom ? new Date(custom) : null);
  const valid = $derived(!!customDate && !isNaN(customDate.getTime()) && customDate.getTime() > Date.now());

  function pick(d: Date) {
    onpick(d);
    onclose();
  }

  function onkeydown(e: KeyboardEvent) {
    // Number keys pick a preset.
    const n = Number(e.key);
    if (!e.metaKey && !e.ctrlKey && n >= 1 && n <= presets.length && !(e.target instanceof HTMLInputElement)) {
      e.preventDefault();
      pick(presets[n - 1].at);
    }
  }
</script>

<Modal label={title} {onclose} {onkeydown} small>
  <div class="head">{title}</div>
  <div class="list">
    {#each presets as p, i (p.label)}
      <button class="choice" onclick={() => pick(p.at)}>
        <span class="key">{i + 1}</span>
        <span class="label">{p.label}</span>
        <span class="when">{whenLabel(p.at)}</span>
      </button>
    {/each}
  </div>
  <form class="custom" onsubmit={(e) => { e.preventDefault(); if (valid && customDate) pick(customDate); }}>
    <input type="datetime-local" bind:value={custom} min={toLocalInput(new Date())} />
    <button class="btn primary" type="submit" disabled={!valid}>{action}</button>
  </form>
</Modal>

<style>
  .head { padding: 16px 18px 8px; font-weight: 600; font-size: 14px; }
  .list { display: flex; flex-direction: column; padding: 0 8px; }
  .choice {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 36px;
    padding: 0 10px;
    border: 0;
    border-radius: 7px;
    background: transparent;
    text-align: left;
    color: var(--text);
    font-size: 13.5px;
  }
  .choice:hover, .choice:focus-visible { background: var(--hover); outline: none; }
  .key {
    width: 18px;
    height: 18px;
    display: grid;
    place-items: center;
    border-radius: 4px;
    border: 1px solid var(--line-strong);
    color: var(--muted);
    font-size: 11px;
  }
  .label { flex: 1; }
  .when { color: var(--muted); font-size: 12.5px; font-variant-numeric: tabular-nums; }
  .custom { display: flex; gap: 8px; padding: 12px 18px 16px; margin-top: 6px; border-top: 1px solid var(--line); }
  .custom input {
    flex: 1;
    min-width: 0;
    height: 32px;
    padding: 0 8px;
    border: 1px solid var(--line-strong);
    border-radius: 7px;
    background: var(--panel);
    color: var(--text);
    font: inherit;
    font-size: 13px;
  }
</style>
