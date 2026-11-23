<script lang="ts">
  import { onMount } from "svelte";
  import ModelPicker from "./ModelPicker.svelte";
  import { ai, checkKey, saveKey, setModel, setPrivateOnly, apiKey, type KeyCheck } from "../ai/config.svelte";
  import { api } from "../api";

  let keyInput = $state("");
  let editing = $state(false);
  let busy = $state(false);
  let status = $state<KeyCheck | null>(null);

  onMount(async () => {
    if (!ai.hasKey) return;
    try {
      status = await checkKey(await apiKey());
    } catch {}
  });

  async function save() {
    const key = keyInput.trim();
    if (!key) return;
    busy = true;
    status = await checkKey(key);
    if (status.ok) {
      try {
        await saveKey(key);
        keyInput = "";
        editing = false;
      } catch (e) {
        status = { ok: false, error: String(e) };
      }
    }
    busy = false;
  }

  async function remove() {
    await saveKey("");
    status = null;
    editing = false;
  }

  const usage = (s: KeyCheck) =>
    s.ok
      ? [s.label, s.usage == null ? null : s.limit == null ? `$${s.usage.toFixed(2)} used` : `$${s.usage.toFixed(2)} of $${s.limit.toFixed(2)} used`]
          .filter(Boolean)
          .join(" · ")
      : "";
</script>

<div class="group">
  <div class="gname">OpenRouter</div>
  <div class="row">
    <span class="text">
      <span class="label">API key</span>
      <span class="detail">
        {#if ai.hasKey && !editing}
          {#if status && !status.ok}<span class="err">{status.error}</span>{:else}{status ? usage(status) : "Saved in Keychain"}{/if}
        {:else}
          Get one at <button class="link" onclick={() => api.openUrl("https://openrouter.ai/keys")}>openrouter.ai/keys</button>
        {/if}
      </span>
    </span>
    {#if ai.hasKey && !editing}
      <button class="btn" onclick={() => (editing = true)}>Change</button>
      <button class="btn danger" onclick={remove}>Remove</button>
    {/if}
  </div>
  {#if !ai.hasKey || editing}
    <div class="keyrow">
      <input
        class="field"
        type="password"
        placeholder="sk-or-…"
        bind:value={keyInput}
        onkeydown={(e) => e.key === "Enter" && save()}
        spellcheck="false"
        autocomplete="off" />
      <button class="btn primary" disabled={busy || !keyInput.trim()} onclick={save}>{busy ? "Checking…" : "Save"}</button>
      {#if editing}<button class="btn" onclick={() => { editing = false; keyInput = ""; }}>Cancel</button>{/if}
    </div>
    {#if status && !status.ok}<div class="err below">{status.error}</div>{/if}
  {/if}
</div>

<div class="group" class:off={!ai.hasKey}>
  <div class="gname">Models</div>
  <div class="row">
    <span class="text">
      <span class="label">Search</span>
      <span class="detail">Answers questions about your mail</span>
    </span>
    <ModelPicker value={ai.search} needsTools onpick={(m) => setModel("search", m)} />
  </div>
  <div class="row">
    <span class="text">
      <span class="label">Writing</span>
      <span class="detail">Rephrases your drafts</span>
    </span>
    <ModelPicker value={ai.write} onpick={(m) => setModel("write", m)} />
  </div>
</div>

<div class="group" class:off={!ai.hasKey}>
  <div class="gname">Privacy</div>
  <label class="row">
    <input type="checkbox" checked={ai.privateOnly} onchange={(e) => setPrivateOnly(e.currentTarget.checked)} />
    <span class="text">
      <span class="label">Skip providers that store or train on data</span>
      <span class="detail">Fewer models available</span>
    </span>
  </label>
  <p class="note">
    Your questions, drafts and the mail the AI reads go to OpenRouter and the model's provider. The AI can't send, move or delete mail.
  </p>
</div>

<style>
  .gname { font-size: 12px; font-weight: 600; color: var(--muted); margin-bottom: 4px; }
  .row {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 0;
    border-bottom: 1px solid var(--line);
    cursor: default;
  }
  label.row { align-items: flex-start; }
  label.row input { margin: 2px 0 0; accent-color: var(--accent); }
  .text { flex: 1; display: flex; flex-direction: column; gap: 2px; min-width: 0; }
  .group + .group { margin-top: 22px; }
  .group.off { opacity: 0.5; pointer-events: none; }
  .label { font-size: 13.5px; }
  .detail { font-size: 12.5px; color: var(--muted); }
  .keyrow { display: flex; gap: 8px; padding: 10px 0; }
  .keyrow .field { flex: 1; height: 32px; font-size: 13px; }
  .err { color: var(--danger); }
  .below { font-size: 12.5px; margin-top: -4px; }
  .link { border: 0; padding: 0; background: none; color: var(--accent); font-size: inherit; cursor: pointer; }
  .note { margin: 12px 0 0; font-size: 12.5px; line-height: 1.5; color: var(--muted); }
</style>
