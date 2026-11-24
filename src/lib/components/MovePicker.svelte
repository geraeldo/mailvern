<script lang="ts">
  import Modal from "./Modal.svelte";
  import Icon from "./Icon.svelte";
  import type { Folder } from "../api";
  import { ui, accountById, accountLabel } from "../state.svelte";

  let { accountId, exclude, onpick, onclose }: {
    accountId: number;
    /** Folders the messages are already in. */
    exclude: number[];
    onpick: (f: Folder) => void;
    onclose: () => void;
  } = $props();

  const roleIcon: Record<string, string> = { inbox: "inbox", sent: "sent", drafts: "drafts", archive: "archive", trash: "trash", junk: "junk" };

  let filter = $state("");
  let active = $state(0);
  let input = $state<HTMLInputElement>();
  let listEl = $state<HTMLDivElement>();

  const folders = $derived(
    ui.folders.filter((f) => {
      if (f.accountId !== accountId || exclude.includes(f.id) || f.role === "snoozed") return false;
      const q = filter.trim().toLowerCase();
      return !q || f.name.toLowerCase().includes(q) || f.path.toLowerCase().includes(q);
    }),
  );
  const acc = $derived(accountById(accountId));

  $effect(() => {
    input?.focus();
  });

  function pick(f: Folder | undefined) {
    if (!f) return;
    onpick(f);
    onclose();
  }

  function scrollActive() {
    queueMicrotask(() => listEl?.querySelectorAll<HTMLElement>(".choice")[active]?.scrollIntoView({ block: "nearest" }));
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      if (!folders.length) return;
      active = Math.min(folders.length - 1, active + 1);
      scrollActive();
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      if (!folders.length) return;
      active = Math.max(0, active - 1);
      scrollActive();
    } else if (e.key === "Enter") {
      e.preventDefault();
      pick(folders[active]);
    }
  }
</script>

<Modal label="Move to folder" {onclose} {onkeydown} small>
  <div class="head">
    <span>Move to</span>
    {#if acc && ui.accounts.length > 1}<span class="acc">{accountLabel(acc)}</span>{/if}
  </div>
  <div class="filter">
    <Icon name="search" size={14} />
    <input bind:this={input} bind:value={filter} oninput={() => (active = 0)} placeholder="Folder name" spellcheck="false" />
  </div>
  <div class="list scroll" bind:this={listEl}>
    {#each folders as f, i (f.id)}
      <button class="choice" class:active={i === active} onmouseenter={() => (active = i)} onclick={() => pick(f)}>
        <Icon name={roleIcon[f.role ?? ""] ?? "folder"} size={15} />
        <span class="label">{f.path}</span>
      </button>
    {:else}
      <div class="none">No folders match</div>
    {/each}
  </div>
</Modal>

<style>
  .head { display: flex; align-items: baseline; gap: 8px; padding: 16px 18px 8px; font-weight: 600; font-size: 14px; }
  .acc { color: var(--muted); font-weight: 400; font-size: 12.5px; }
  .filter {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 32px;
    margin: 0 14px 8px;
    padding: 0 10px;
    border-radius: 8px;
    background: var(--hover);
    color: var(--muted);
  }
  .filter input { flex: 1; min-width: 0; border: 0; outline: none; background: transparent; color: var(--text); font: inherit; font-size: 13px; }
  .list { display: flex; flex-direction: column; padding: 0 8px 10px; overflow-y: auto; min-height: 0; }
  .choice {
    flex: none;
    display: flex;
    align-items: center;
    gap: 10px;
    height: 32px;
    padding: 0 10px;
    border: 0;
    border-radius: 7px;
    background: transparent;
    text-align: left;
    color: var(--text);
    font-size: 13.5px;
  }
  .choice :global(svg) { color: var(--muted); flex: none; }
  .choice.active { background: var(--hover); }
  .label { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .none { padding: 16px 10px; color: var(--muted); font-size: 13px; }
</style>
