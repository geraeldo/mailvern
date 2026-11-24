<script lang="ts">
  import { ask } from "@tauri-apps/plugin-dialog";
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

  async function confirmReset() {
    const ok = await ask("Restore all keyboard shortcuts to their defaults?", {
      title: "Reset shortcuts?",
      kind: "warning",
      okLabel: "Reset",
      cancelLabel: "Cancel",
    });
    if (!ok) return;
    await resetShortcuts();
    toast("Shortcuts reset to defaults");
  }

  $effect(() => () => (keymap.recording = false));
</script>

<svelte:window onkeydowncapture={onKey} />

<div class="wrap">
  {#each groups as g (g)}
    <div class="group">
      <div class="gname">{g}</div>
      <div class="card">
        {#each ACTIONS.filter((a) => a.group === g) as a (a.id)}
          <div class="row">
            <span class="label">{a.label}</span>
            <span class="keys">
              {#each keymap.bindings[a.id] as c (c)}
                <span class="kbd">
                  {display(c)}
                  <button type="button" aria-label={`Remove ${display(c)}`} onclick={() => unbind(a.id, c)}>
                    <Icon name="x" size={10} stroke={2.2} />
                  </button>
                </span>
              {/each}
              {#if recordingFor === a.id}
                <span class="kbd rec">Press keys…</span>
              {:else}
                <button type="button" class="add" title={`Add shortcut for ${a.label}`} aria-label={`Add shortcut for ${a.label}`} onclick={() => start(a.id)}>
                  <Icon name="plus" size={13} />
                </button>
              {/if}
            </span>
          </div>
        {/each}
      </div>
    </div>
  {/each}
  <div class="foot">
    <span class="tip">Press <span class="mini-kbd">?</span> anywhere in Mailvern for a quick shortcut guide.</span>
    <button type="button" class="btn ghost sm" onclick={confirmReset}>Reset to defaults</button>
  </div>
</div>

<style>
  .wrap { display: flex; flex-direction: column; gap: 22px; }
  .gname { font-size: 12px; font-weight: 600; color: var(--muted); margin-bottom: 8px; }
  .card {
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--panel);
  }
  .row {
    display: flex;
    align-items: center;
    gap: 12px;
    min-height: 42px;
    padding: 6px 14px;
  }
  .row + .row { border-top: 1px solid var(--line); }
  .row:hover { background: var(--hover); }
  .row:first-child { border-radius: var(--radius) var(--radius) 0 0; }
  .row:last-child { border-radius: 0 0 var(--radius) var(--radius); }
  .row:only-child { border-radius: var(--radius); }
  .label { flex: 1; font-size: 13.5px; color: var(--text); }
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
    display: grid;
    place-items: center;
    width: 16px;
    height: 16px;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: var(--text);
    color: var(--panel);
    opacity: 0;
    pointer-events: none;
    transition: opacity 0.12s;
  }
  .kbd:hover button, .kbd:focus-within button { opacity: 1; pointer-events: auto; }
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
    opacity: 0.3;
    transition: opacity 0.12s, color 0.12s, border-color 0.12s;
  }
  .row:hover .add, .row:focus-within .add { opacity: 1; }
  .add:hover { color: var(--text); border-color: var(--muted); }
  .foot { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
  .tip { font-size: 12px; color: var(--muted); }
  .mini-kbd {
    display: inline-grid;
    place-items: center;
    min-width: 18px;
    height: 18px;
    padding: 0 5px;
    border-radius: 4px;
    border: 1px solid var(--line-strong);
    background: var(--field);
    color: var(--text-2);
    font-size: 11px;
    font-weight: 600;
  }
</style>
