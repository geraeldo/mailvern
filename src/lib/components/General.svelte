<script lang="ts">
  import { BADGE_MODES, badge, setBadgeMode } from "../badge.svelte";
  import { ui, setUndoSend, setThreaded, setSenderIcons, untrustSender } from "../state.svelte";
  import { THEME_MODES, theme, setThemeMode } from "../theme.svelte";
  import Select from "./Select.svelte";
  import Switch from "./Switch.svelte";
  import Icon from "./Icon.svelte";

  const UNDO = [0, 5, 10, 20, 30];
</script>

<div class="group">
  <div class="gname">Appearance</div>
  <div class="card">
    <div class="row">
      <span class="text">
        <span class="label">Theme</span>
        <span class="detail">System follows your Mac's light or dark setting</span>
      </span>
      <Select label="Theme" value={theme.mode} options={THEME_MODES} onchange={setThemeMode} />
    </div>
  </div>
</div>

<div class="group">
  <div class="gname">Messages</div>
  <div class="card">
    <label class="row interactive">
      <span class="text">
        <span class="label">Group into conversations</span>
        <span class="detail">Show replies together in one row</span>
      </span>
      <Switch checked={ui.threaded} label="Group into conversations" onchange={(v) => setThreaded(v)} />
    </label>
    <label class="row interactive">
      <span class="text">
        <span class="label">Show sender logos</span>
        <span class="detail">Fetched from the sender's website</span>
      </span>
      <Switch checked={ui.senderIcons} label="Show sender logos" onchange={(v) => setSenderIcons(v)} />
    </label>
    <div class="row">
      <span class="text">
        <span class="label">Undo send</span>
        <span class="detail">Time to take back a sent message</span>
      </span>
      <Select
        label="Undo send"
        value={ui.undoSend}
        options={UNDO.map((s) => ({ value: s, label: s === 0 ? "Off" : `${s} seconds` }))}
        onchange={setUndoSend} />
    </div>
  </div>
</div>

<div class="group">
  <div class="gname">Dock badge</div>
  <div class="card">
    {#each BADGE_MODES as m (m.id)}
      <label class="row interactive">
        <input type="radio" name="badge" value={m.id} checked={badge.mode === m.id} onchange={() => setBadgeMode(m.id)} />
        <span class="text">
          <span class="label">{m.label}</span>
          {#if m.detail}<span class="detail">{m.detail}</span>{/if}
        </span>
      </label>
    {/each}
  </div>
</div>

{#if ui.trustedSenders.length}
  <div class="group">
    <div class="gname">Remote images · Trusted senders</div>
    <div class="card">
      {#each ui.trustedSenders as email (email)}
        <div class="row trusted">
          <span class="label addr">{email}</span>
          <button type="button" class="icon-btn sm" title={`Stop always loading images from ${email}`} aria-label={`Remove ${email}`} onclick={() => untrustSender(email)}>
            <Icon name="x" size={14} />
          </button>
        </div>
      {/each}
    </div>
    <p class="hint">Images from these senders load automatically without asking.</p>
  </div>
{/if}

<style>
  .gname { font-size: 12px; font-weight: 600; color: var(--muted); margin-bottom: 8px; }
  .group + .group { margin-top: 24px; }
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
  .row.trusted { padding: 7px 8px 7px 14px; }
  input[type="radio"] { margin: 0; accent-color: var(--accent); flex: none; }
  .text { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
  .label { font-size: 13.5px; color: var(--text); }
  .addr { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .detail { font-size: 12.5px; color: var(--muted); }
  .hint { margin: 8px 0 0; font-size: 12px; color: var(--muted); }
  .icon-btn.sm { width: 26px; height: 26px; flex: none; }
</style>
