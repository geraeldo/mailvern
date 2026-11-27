<script lang="ts">
  import { onMount } from "svelte";
  import { registerModal } from "../modal-stack";

  let {
    title,
    placeholder = "",
    initial = "",
    action = "Save",
    onsubmit,
    onclose,
  }: {
    title: string;
    placeholder?: string;
    initial?: string;
    action?: string;
    onsubmit: (value: string) => void | Promise<void>;
    onclose: () => void;
  } = $props();

  let value = $state("");
  let busy = $state(false);
  let inputEl: HTMLInputElement;

  $effect(() =>
    registerModal(() => {
      if (!busy) onclose();
    }),
  );

  onMount(() => {
    value = initial;
    queueMicrotask(() => {
      inputEl?.focus();
      inputEl?.select();
    });
  });

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    const v = value.trim();
    if (!v || busy) return;
    busy = true;
    try {
      await onsubmit(v);
      onclose();
    } finally {
      busy = false;
    }
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="scrim"
  onmousedown={(e) => { if (e.target === e.currentTarget && !busy) onclose(); }}
  onkeydown={(e) => { if (e.key === "Escape" && !busy) { e.preventDefault(); onclose(); } }}>
  <div class="box" role="dialog" tabindex="-1" aria-label={title}>
    <form class="form" onsubmit={submit}>
      <h3>{title}</h3>
      <input bind:this={inputEl} bind:value {placeholder} disabled={busy} />
      <div class="foot">
        <button type="button" class="btn sm" disabled={busy} onclick={onclose}>Cancel</button>
        <button type="submit" class="btn primary sm" disabled={busy || !value.trim()}>{busy ? "Saving…" : action}</button>
      </div>
    </form>
  </div>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 70;
    display: grid;
    place-items: center;
    background: rgba(20, 18, 14, 0.22);
    animation: fade 0.1s ease-out;
  }
  .box {
    width: min(360px, calc(100vw - 48px));
    padding: 18px 20px 16px;
    background: var(--panel);
    border: 1px solid var(--sheet-border);
    border-radius: 12px;
    box-shadow: var(--shadow);
  }
  .form {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  h3 {
    margin: 0;
    font-size: 15px;
    font-weight: 600;
    color: var(--text);
  }
  input {
    height: 34px;
    padding: 0 10px;
    border: 1px solid var(--line-strong);
    border-radius: 8px;
    background: var(--bg);
    color: var(--text);
    font-size: 13.5px;
    outline: none;
  }
  input:focus { border-color: var(--accent); }
  .foot {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 2px;
  }
</style>
