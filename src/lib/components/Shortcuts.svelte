<script lang="ts">
  import Icon from "./Icon.svelte";
  import { ACTIONS, keymap, bind, unbind, resetShortcuts, comboOf, display, labelOf, type Action } from "../shortcuts.svelte";
  import { toast } from "../state.svelte";

  let recordingFor = $state<Action | null>(null);
  const groups = [...new Set(ACTIONS.map((a) => a.group))];

  function start(a: Action) {
    recordingFor = a;
    keymap.recording = true;
  }

  function stop() {
    recordingFor = null;
    keymap.recording = false;
  }

  async function onKey(e: KeyboardEvent) {
    if (!recordingFor) return;
    e.preventDefault();
    e.stopPropagation();
    if (e.key === "Escape") return stop();
    const combo = comboOf(e);
    if (!combo) return;
    const action = recordingFor;
    stop();
    const taken = await bind(action, combo);
    if (taken) toast(`${display(combo)} moved from ${labelOf(taken)}`);
  }

  $effect(() => () => (keymap.recording = false));
</script>

<svelte:window onkeydowncapture={onKey} />

<div class="wrap">
  {#each groups as g (g)}
    <div class="group">
      <div class="gname">{g}</div>
      {#each ACTIONS.filter((a) => a.group === g) as a (a.id)}
        <div class="row">
          <span class="label">{a.label}</span>
          <span class="keys">
            {#each keymap.bindings[a.id] as c (c)}
              <span class="kbd">
                {display(c)}
                <button aria-label="Remove" onclick={() => unbind(a.id, c)}><Icon name="x" size={10} stroke={2.2} /></button>
              </span>
            {/each}
            {#if recordingFor === a.id}
              <span class="kbd rec">Press keys</span>
            {:else}
              <button class="add" aria-label="Add shortcut" onclick={() => start(a.id)}><Icon name="plus" size={13} /></button>
            {/if}
          </span>
        </div>
      {/each}
    </div>
  {/each}
  <div class="foot">
    <button class="btn ghost" onclick={() => resetShortcuts()}>Reset to defaults</button>
  </div>
</div>

<style>
  .wrap { display: flex; flex-direction: column; gap: 22px; }
  .gname { font-size: 12px; font-weight: 600; color: var(--muted); margin-bottom: 4px; }
  .row {
    display: flex;
    align-items: center;
    gap: 12px;
    min-height: 40px;
    border-bottom: 1px solid var(--line);
  }
  .label { flex: 1; font-size: 13.5px; }
  .keys { display: flex; align-items: center; gap: 6px; flex-wrap: wrap; justify-content: flex-end; }
  .kbd {
    position: relative;
    display: inline-flex;
    align-items: center;
    height: 24px;
    min-width: 24px;
    padding: 0 8px;
    border-radius: 6px;
    border: 1px solid var(--line-strong);
    border-bottom-width: 2px;
    background: var(--field);
    font: 500 12px/1 var(--font);
    color: var(--text-2);
    justify-content: center;
  }
  .kbd button {
    position: absolute;
    top: -7px;
    right: -7px;
    display: none;
    place-items: center;
    width: 16px;
    height: 16px;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: var(--text);
    color: var(--panel);
  }
  .kbd:hover button { display: grid; }
  .kbd.rec { border-color: var(--accent); color: var(--accent); animation: fade 0.6s ease-in-out infinite alternate; }
  .add {
    display: grid;
    place-items: center;
    width: 24px;
    height: 24px;
    border: 1px dashed var(--line-strong);
    border-radius: 6px;
    background: transparent;
    color: var(--muted);
    opacity: 0;
    transition: opacity 0.12s;
  }
  .row:hover .add { opacity: 1; }
  .add:hover { color: var(--text); border-color: var(--muted); }
  .foot { display: flex; justify-content: flex-end; }
</style>
