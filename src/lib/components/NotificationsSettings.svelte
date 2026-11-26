<script lang="ts">
  import { BADGE_MODES, badge, setBadgeMode } from "../badge.svelte";
  import { ui, accountLabel, setAccountAlerts } from "../state.svelte";
  import type { AccountBadgeMode } from "../api";
  import Select from "./Select.svelte";
  import Switch from "./Switch.svelte";

  const BADGE_OPTIONS: { value: AccountBadgeMode; label: string }[] = [
    { value: "", label: "Default" },
    { value: "new", label: "New" },
    { value: "all", label: "All unread" },
    { value: "off", label: "Off" },
  ];
</script>

<div class="group">
  <div class="gname">Default Dock badge</div>
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

{#if ui.accounts.length}
  <div class="group">
    <div class="gname">Per account</div>
    <div class="card">
      <div class="tbl-head">
        <span>Account</span>
        <span>Dock badge</span>
        <span class="col-right">Banners</span>
      </div>
      {#each ui.accounts as acc (acc.id)}
        {@const title = accountLabel(acc)}
        {@const hasSub = title.toLowerCase() !== acc.email.toLowerCase()}
        <div class="tbl-row">
          <div class="acc-cell">
            <span class="acc-bar" style:background={acc.color || "var(--muted)"}></span>
            <span class="text">
              <span class="label">{title}</span>
              {#if hasSub}<span class="detail">{acc.email}</span>{/if}
            </span>
          </div>
          <div class="badge-cell">
            <Select
              label={`Dock badge for ${acc.email}`}
              value={acc.badgeMode ?? ""}
              options={BADGE_OPTIONS}
              onchange={(v) => setAccountAlerts(acc.id, acc.notify ?? true, v)} />
          </div>
          <div class="switch-cell">
            <Switch
              checked={acc.notify ?? true}
              label={`Banner notifications for ${acc.email}`}
              onchange={(on) => setAccountAlerts(acc.id, on, acc.badgeMode ?? "")} />
          </div>
        </div>
      {/each}
    </div>
    <p class="hint">
      Banners show macOS notifications when new mail arrives. Dock badge controls how each account counts toward Mailvern’s Dock icon.
    </p>
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
  input[type="radio"] { margin: 0; accent-color: var(--accent); flex: none; }

  .tbl-head, .tbl-row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 124px 68px;
    align-items: center;
    gap: 12px;
    padding: 0 14px;
  }
  .tbl-head {
    height: 32px;
    background: var(--bg);
    border-bottom: 1px solid var(--line);
    font-size: 11.5px;
    font-weight: 600;
    color: var(--muted);
  }
  .col-right { text-align: right; }
  .tbl-row {
    min-height: 46px;
    padding-top: 8px;
    padding-bottom: 8px;
  }
  .tbl-row + .tbl-row { border-top: 1px solid var(--line); }
  .acc-cell {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
  }
  .acc-bar { width: 3px; height: 20px; border-radius: 2px; flex: none; }
  .badge-cell { display: flex; }
  .badge-cell :global(.select) { width: 100%; }
  .switch-cell { display: flex; justify-content: flex-end; }

  .text { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 1px; }
  .label { font-size: 13.5px; color: var(--text); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .detail { font-size: 12px; color: var(--muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .hint { margin: 8px 0 0; font-size: 12px; color: var(--muted); line-height: 1.45; }
</style>
