<script lang="ts">
  import type { Snippet } from "svelte";
  import { keymap } from "../shortcuts.svelte";

  let { label, onclose, onkeydown, children, small = false, medium = false }: {
    label: string;
    onclose: () => void;
    onkeydown?: (e: KeyboardEvent) => void;
    children: Snippet;
    /** A small dialog sized to its content, instead of the settings sheet. */
    small?: boolean;
    /** A medium dialog (e.g. shortcuts cheat sheet). */
    medium?: boolean;
  } = $props();

  let sheet = $state<HTMLDivElement>();
  // Only a press that starts and ends on the backdrop closes the sheet, so
  // selecting text in a field and releasing outside doesn't dismiss it.
  let downOnScrim = false;

  $effect(() => {
    if (sheet && !sheet.contains(document.activeElement)) sheet.focus();
  });

  function onKey(e: KeyboardEvent) {
    onkeydown?.(e);
    if (e.key === "Escape" && !keymap.recording && !e.defaultPrevented) onclose();
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
<div
  class="scrim"
  onkeydown={onKey}
  onmousedown={(e) => (downOnScrim = e.target === e.currentTarget)}
  onclick={(e) => {
    if (downOnScrim && e.target === e.currentTarget) onclose();
    downOnScrim = false;
  }}>
  <div class="sheet" class:small class:medium role="dialog" aria-label={label} tabindex="-1" bind:this={sheet}>
    {@render children()}
  </div>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 60;
    display: grid;
    place-items: center;
    background: rgba(20, 18, 14, 0.24);
    animation: fade 0.12s ease-out;
  }
  .sheet {
    display: flex;
    width: min(920px, calc(100vw - 48px));
    height: min(720px, calc(100vh - 64px));
    background: var(--panel);
    border: 1px solid var(--sheet-border);
    border-radius: 14px;
    box-shadow: var(--shadow);
    overflow: hidden;
    outline: none;
    animation: rise 0.16s cubic-bezier(0.2, 0.8, 0.2, 1);
  }
  .sheet.small {
    flex-direction: column;
    width: min(350px, calc(100vw - 48px));
    height: auto;
    max-height: min(520px, calc(100vh - 64px));
  }
  .sheet.medium {
    flex-direction: column;
    width: min(580px, calc(100vw - 48px));
    height: auto;
    max-height: min(640px, calc(100vh - 64px));
  }
</style>
