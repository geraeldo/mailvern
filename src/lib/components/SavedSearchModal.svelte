<script lang="ts">
  import { onMount } from "svelte";
  import Modal from "./Modal.svelte";
  import Select from "./Select.svelte";
  import type { SearchScope } from "../api";
  import { ui, accountLabel, scopeKey, scopeLabel } from "../state.svelte";

  let {
    id,
    initialName,
    initialQuery,
    initialScope,
    onsubmit,
    ondelete,
    onclose,
  }: {
    id: number;
    initialName: string;
    initialQuery: string;
    initialScope: SearchScope;
    onsubmit: (id: number, name: string, query: string, scope: SearchScope) => Promise<void> | void;
    ondelete?: () => void;
    onclose: () => void;
  } = $props();

  let name = $state("");
  let query = $state("");
  let scope = $state<SearchScope>({ kind: "all" });
  let busy = $state(false);
  let error = $state<string | null>(null);
  let nameEl = $state<HTMLInputElement>();

  const title = $derived(id > 0 ? "Edit saved search" : "Save search");

  onMount(() => {
    name = initialName;
    query = initialQuery;
    scope = initialScope;
    queueMicrotask(() => {
      nameEl?.focus();
      nameEl?.select();
    });
  });

  const scopeOptions = $derived.by(() => {
    const list: { value: string; label: string; scope: SearchScope }[] = [
      { value: "all", label: "All Mailboxes", scope: { kind: "all" } },
    ];
    for (const a of ui.accounts) {
      const s: SearchScope = { kind: "account", accountId: a.id };
      list.push({ value: scopeKey(s), label: accountLabel(a), scope: s });
    }
    const curKey = scopeKey(scope);
    if (!list.some((o) => o.value === curKey)) {
      list.push({ value: curKey, label: scopeLabel(scope), scope });
    }
    return list;
  });

  function pickScope(val: string) {
    const found = scopeOptions.find((o) => o.value === val);
    if (found) scope = found.scope;
  }

  async function submit(e: Event) {
    e.preventDefault();
    if (busy) return;
    const n = name.trim();
    const q = query.trim();
    if (!n) {
      error = "Give the saved search a name";
      return;
    }
    if (!q) {
      error = "Enter a search query";
      return;
    }
    busy = true;
    error = null;
    try {
      await onsubmit(id, n, q, scope);
    } catch (err) {
      error = String(err);
    } finally {
      busy = false;
    }
  }
</script>

<Modal label={title} small {onclose}>
  <form class="body" onsubmit={submit}>
    <h3>{title}</h3>
    <label>
      <span>Name</span>
      <input bind:this={nameEl} class="field" bind:value={name} placeholder="e.g. Unread invoices" />
    </label>
    <label>
      <span>Query</span>
      <input class="field mono" bind:value={query} placeholder='e.g. (invoice OR receipt) -from:promo' spellcheck="false" />
    </label>
    <p class="hint">Supports <code>OR</code>, <code>(...)</code>, <code>-exclude</code>, <code>"phrase"</code>, <code>from:</code>, <code>filename:</code>, <code>larger:5M</code>, <code>smaller:100K</code>, <code>after:7d</code>.</p>
    <div class="scope-row">
      <span class="lbl">Search in</span>
      <Select label="Search scope" value={scopeKey(scope)} options={scopeOptions} onchange={pickScope} />
    </div>
    {#if error}<div class="err">{error}</div>{/if}
    <footer>
      {#if id > 0 && ondelete}
        <button type="button" class="btn ghost danger" disabled={busy} onclick={ondelete}>Delete</button>
      {/if}
      <span class="grow"></span>
      <button type="button" class="btn ghost" disabled={busy} onclick={onclose}>Cancel</button>
      <button type="submit" class="btn primary" disabled={busy || !name.trim() || !query.trim()}>
        {busy ? "Saving…" : "Save"}
      </button>
    </footer>
  </form>
</Modal>

<style>
  .body { padding: 16px 18px 16px; display: flex; flex-direction: column; gap: 12px; }
  h3 { margin: 0 0 2px; font-size: 15px; font-weight: 600; color: var(--text); }
  label { display: flex; flex-direction: column; gap: 5px; }
  label span, .lbl { font-size: 12.5px; font-weight: 500; color: var(--text-2); }
  .mono { font-family: ui-monospace, "SF Mono", Menlo, monospace; font-size: 12.5px; }
  .hint { margin: -4px 0 0; font-size: 11.5px; color: var(--muted); line-height: 1.45; }
  .hint code { font-family: ui-monospace, "SF Mono", Menlo, monospace; font-size: 11px; color: var(--text-2); }
  .scope-row { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
  .err { padding: 8px 10px; border-radius: 7px; background: var(--danger-soft); color: var(--danger); font-size: 12.5px; }
  footer { display: flex; align-items: center; gap: 8px; padding-top: 4px; }
  .grow { flex: 1; }
</style>
