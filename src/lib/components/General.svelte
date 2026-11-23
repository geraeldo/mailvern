<script lang="ts">
  import { BADGE_MODES, badge, setBadgeMode } from "../badge.svelte";
  import { ui, setUndoSend, setThreaded, setSenderIcons } from "../state.svelte";
  import { THEME_MODES, theme, setThemeMode } from "../theme.svelte";
  import Select from "./Select.svelte";

  const UNDO = [0, 5, 10, 20, 30];
</script>

<div class="group">
  <div class="gname">Appearance</div>
  <div class="row">
    <span class="text">
      <span class="label">Theme</span>
      <span class="detail">System follows your Mac's light or dark setting</span>
    </span>
    <Select label="Theme" value={theme.mode} options={THEME_MODES} onchange={setThemeMode} />
  </div>
</div>

<div class="group">
  <div class="gname">Messages</div>
  <label class="row">
    <input type="checkbox" checked={ui.threaded} onchange={(e) => setThreaded(e.currentTarget.checked)} />
    <span class="text">
      <span class="label">Group into conversations</span>
      <span class="detail">Show replies together in one row</span>
    </span>
  </label>
  <label class="row">
    <input type="checkbox" checked={ui.senderIcons} onchange={(e) => setSenderIcons(e.currentTarget.checked)} />
    <span class="text">
      <span class="label">Show sender logos</span>
      <span class="detail">Fetched from the sender's website</span>
    </span>
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

<div class="group">
  <div class="gname">Dock badge</div>
  {#each BADGE_MODES as m (m.id)}
    <label class="row">
      <input type="radio" name="badge" value={m.id} checked={badge.mode === m.id} onchange={() => setBadgeMode(m.id)} />
      <span class="text">
        <span class="label">{m.label}</span>
        {#if m.detail}<span class="detail">{m.detail}</span>{/if}
      </span>
    </label>
  {/each}
</div>

<style>
  .gname { font-size: 12px; font-weight: 600; color: var(--muted); margin-bottom: 4px; }
  .row {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 10px 0;
    border-bottom: 1px solid var(--line);
    cursor: default;
  }
  input { margin: 2px 0 0; accent-color: var(--accent); }
  .text { flex: 1; display: flex; flex-direction: column; gap: 2px; }
  .group + .group { margin-top: 22px; }
  .label { font-size: 13.5px; }
  .detail { font-size: 12.5px; color: var(--muted); }
</style>
