<script lang="ts">
  import { onMount } from "svelte";
  import { ask } from "@tauri-apps/plugin-dialog";
  import ModelPicker from "./ModelPicker.svelte";
  import Switch from "./Switch.svelte";
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
    const ok = await ask("Remove your OpenRouter API key? AI search, rephrasing and rule generation will stop until you add a key again.", {
      title: "Remove API key?",
      kind: "warning",
      okLabel: "Remove",
      cancelLabel: "Cancel",
    });
    if (!ok) return;
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
  <div class="card">
    <div class="row">
      <span class="text">
        <span class="label">API key</span>
        <span class="detail">
          {#if ai.hasKey && !editing}
            {#if status && !status.ok}<span class="err">{status.error}</span>{:else}{status ? usage(status) : "Saved in Keychain"}{/if}
          {:else}
            Get one at <button type="button" class="link" onclick={() => api.openUrl("https://openrouter.ai/keys")}>openrouter.ai/keys</button>
          {/if}
        </span>
      </span>
      {#if ai.hasKey && !editing}
        <button type="button" class="btn sm" onclick={() => (editing = true)}>Change</button>
        <button type="button" class="btn sm ghost danger" onclick={remove}>Remove</button>
      {/if}
    </div>
    {#if !ai.hasKey || editing}
      <div class="keybox">
        <div class="keyrow">
          <input
            class="field"
            type="password"
            placeholder="sk-or-…"
            bind:value={keyInput}
            onkeydown={(e) => e.key === "Enter" && save()}
            spellcheck="false"
            autocomplete="off" />
          <button type="button" class="btn primary sm" disabled={busy || !keyInput.trim()} onclick={save}>{busy ? "Checking…" : "Save"}</button>
          {#if editing}<button type="button" class="btn sm" onclick={() => { editing = false; keyInput = ""; }}>Cancel</button>{/if}
        </div>
        {#if status && !status.ok}<div class="err below">{status.error}</div>{/if}
      </div>
    {/if}
  </div>
</div>

<div class="group" class:off={!ai.hasKey} inert={!ai.hasKey}>
  <div class="gname">Default models</div>
  <div class="card">
    <div class="row">
      <span class="text">
        <span class="label">Search &amp; rules</span>
        <span class="detail">Answers questions about your mail and drafts rules</span>
      </span>
      <ModelPicker value={ai.search} needsTools onpick={(m) => setModel("search", m)} />
    </div>
    <div class="row">
      <span class="text">
        <span class="label">Writing</span>
        <span class="detail">Rephrases your drafts in the composer</span>
      </span>
      <ModelPicker value={ai.write} onpick={(m) => setModel("write", m)} />
    </div>
  </div>
</div>

<div class="group" class:off={!ai.hasKey} inert={!ai.hasKey}>
  <div class="gname">Privacy</div>
  <div class="card">
    <label class="row interactive">
      <span class="text">
        <span class="label">Skip providers that store or train on data</span>
        <span class="detail">Fewer models available</span>
      </span>
      <Switch checked={ai.privateOnly} label="Skip providers that store or train on data" onchange={(v) => setPrivateOnly(v)} />
    </label>
  </div>
  <p class="note">
    Your questions, drafts and the mail the AI reads go to OpenRouter and the model's provider. The AI can't send, move or delete mail.
  </p>
</div>

<style>
  .gname { font-size: 12px; font-weight: 600; color: var(--muted); margin-bottom: 8px; }
  .group + .group { margin-top: 24px; }
  .group.off { opacity: 0.5; }
  .card {
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--panel);
    overflow: hidden;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 11px 14px;
    cursor: default;
  }
  .row + .row { border-top: 1px solid var(--line); }
  .row.interactive:hover { background: var(--hover); }
  .text { flex: 1; display: flex; flex-direction: column; gap: 2px; min-width: 0; }
  .label { font-size: 13.5px; color: var(--text); }
  .detail { font-size: 12.5px; color: var(--muted); }
  .keybox { padding: 0 14px 12px; }
  .keyrow { display: flex; align-items: center; gap: 8px; }
  .keyrow .field { flex: 1; height: 32px; font-size: 13px; }
  .err { color: var(--danger); }
  .below { font-size: 12.5px; margin-top: 8px; }
  .link { border: 0; padding: 0; background: none; color: var(--accent); font-size: inherit; cursor: pointer; }
  .link:hover { text-decoration: underline; }
  .note { margin: 10px 0 0; font-size: 12px; line-height: 1.5; color: var(--muted); }
</style>
