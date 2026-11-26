<script lang="ts">
  import Modal from "./Modal.svelte";
  import Icon from "./Icon.svelte";
  import { ACTIONS, keymap, display } from "../shortcuts.svelte";
  import { openSettings } from "../state.svelte";

  let { onclose }: { onclose: () => void } = $props();

  const groups = [...new Set(ACTIONS.map((a) => a.group))];
  const extras = [
    { label: "Ask AI from search", keys: ["⌘↩"] },
    { label: "Print conversation", keys: ["⌘P"] },
    { label: "Undo last action", keys: ["⌘Z"] },
    { label: "Toggle sidebar", keys: ["⌥⌘S"] },
    { label: "Pick snooze preset", keys: ["1–5"] },
    { label: "Clear selection / Exit AI", keys: ["Esc"] },
  ];
</script>

<Modal label="Keyboard shortcuts" {onclose} medium>
  <header>
    <h2>Keyboard shortcuts</h2>
    <button type="button" class="btn ghost sm" onclick={() => { onclose(); openSettings("shortcuts"); }}>Customize…</button>
    <button type="button" class="icon-btn" title="Close  Esc" onclick={onclose}><Icon name="x" /></button>
  </header>
  <div class="body scroll">
    <div class="cols">
      {#each groups as g (g)}
        <div class="group">
          <h3>{g}</h3>
          {#each ACTIONS.filter((a) => a.group === g) as a (a.id)}
            <div class="row">
              <span class="label">{a.label}</span>
              <span class="keys">
                {#each keymap.bindings[a.id].slice(0, 2) as c (c)}
                  <span class="kbd">{display(c)}</span>
                {/each}
              </span>
            </div>
          {/each}
        </div>
      {/each}
      <div class="group">
        <h3>More</h3>
        {#each extras as ex (ex.label)}
          <div class="row">
            <span class="label">{ex.label}</span>
            <span class="keys">
              {#each ex.keys as k (k)}
                <span class="kbd">{k}</span>
              {/each}
            </span>
          </div>
        {/each}
      </div>
    </div>
  </div>
</Modal>

<style>
  header {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 54px;
    flex: none;
    padding: 0 14px 0 22px;
    border-bottom: 1px solid var(--line);
  }
  h2 { flex: 1; margin: 0; font-size: 16px; font-weight: 650; }
  .body { padding: 18px 22px 22px; }
  .cols { display: grid; grid-template-columns: 1fr 1fr; gap: 20px 28px; }
  h3 { margin: 0 0 8px; font-size: 11.5px; font-weight: 600; color: var(--muted); }
  .row { display: flex; align-items: center; justify-content: space-between; gap: 8px; padding: 5px 0; }
  .label { font-size: 13px; color: var(--text-2); }
  .keys { display: flex; align-items: center; gap: 4px; flex: none; }
  .kbd {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    height: 22px;
    min-width: 22px;
    padding: 0 6px;
    border-radius: 5px;
    border: 1px solid var(--line-strong);
    border-bottom-width: 2px;
    background: var(--field);
    color: var(--text-2);
    font-size: 11.5px;
    font-weight: 500;
  }
</style>
