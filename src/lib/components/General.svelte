<script lang="ts">
  import { BADGE_MODES, badge, setBadgeMode } from "../badge.svelte";
  import { ui, accountLabel, setAccountAlerts, setUndoSend, setThreaded, setSenderIcons, setRemoteImages, setAutoTrustSent, trustSender, untrustSender, type RemoteImagesMode } from "../state.svelte";
  import type { AccountBadgeMode } from "../api";
  import { THEME_MODES, theme, setThemeMode } from "../theme.svelte";
  import Select from "./Select.svelte";
  import Switch from "./Switch.svelte";
  import Icon from "./Icon.svelte";

  const UNDO = [0, 5, 10, 20, 30];
  const REMOTE_MODES: { value: RemoteImagesMode; label: string }[] = [
    { value: "ask", label: "Ask before loading" },
    { value: "strip_trackers", label: "Block trackers only" },
    { value: "always", label: "Always load" },
  ];
  const defaultBadgeLabel = $derived(BADGE_MODES.find((m) => m.id === badge.mode)?.label ?? "New messages");
  const accountBadgeOptions = $derived<{ value: AccountBadgeMode; label: string }[]>([
    { value: "", label: `Default (${defaultBadgeLabel})` },
    { value: "new", label: "New messages" },
    { value: "all", label: "All unread" },
    { value: "off", label: "Off" },
  ]);

  let newTrust = $state("");
  let trustError = $state("");

  async function addTrusted(e: SubmitEvent) {
    e.preventDefault();
    const raw = newTrust.trim();
    if (!raw) return;
    const res = await trustSender(raw);
    if (res.ok) {
      newTrust = "";
      trustError = "";
    } else if (res.reason === "shared_provider") {
      trustError = `${res.domain} is a shared email provider — trust a specific email address instead.`;
    } else {
      trustError = "Enter a valid email address or domain (e.g. alice@example.com or @github.com).";
    }
  }
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
  <div class="gname">Dock badge &amp; notifications</div>
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
  {#if ui.accounts.length}
    <div class="subname">Per account</div>
    <div class="card">
      {#each ui.accounts as acc (acc.id)}
        <div class="row acc-alert">
          <span class="acc-bar" style:background={acc.color || "var(--muted)"}></span>
          <span class="text">
            <span class="label">{accountLabel(acc)}</span>
            <span class="detail">{acc.email}</span>
          </span>
          <Select
            label={`Dock badge for ${acc.email}`}
            value={acc.badgeMode ?? ""}
            options={accountBadgeOptions}
            onchange={(v) => setAccountAlerts(acc.id, acc.notify ?? true, v)} />
          <label class="notif-toggle" title="Desktop notifications">
            <span class="notif-lbl">Alerts</span>
            <Switch
              checked={acc.notify ?? true}
              label={`Notifications for ${acc.email}`}
              onchange={(on) => setAccountAlerts(acc.id, on, acc.badgeMode ?? "")} />
          </label>
        </div>
      {/each}
    </div>
  {/if}
</div>

<div class="group">
  <div class="gname">Remote images</div>
  <div class="card">
    <div class="row">
      <span class="text">
        <span class="label">Load remote images</span>
        <span class="detail">External images can notify senders when you open a message</span>
      </span>
      <Select label="Load remote images" value={ui.remoteImages} options={REMOTE_MODES} onchange={setRemoteImages} />
    </div>
    <label class="row interactive" class:disabled={ui.remoteImages === "always"}>
      <span class="text">
        <span class="label">Auto-trust people you've emailed</span>
        <span class="detail">Load images automatically from Sent recipients and your own accounts</span>
      </span>
      <Switch
        checked={ui.autoTrustSent}
        disabled={ui.remoteImages === "always"}
        label="Auto-trust people you've emailed"
        onchange={(v) => setAutoTrustSent(v)} />
    </label>
  </div>
</div>

<div class="group">
  <div class="gname">Trusted senders & domains</div>
  <div class="card">
    <form class="row add-trusted" onsubmit={addTrusted}>
      <input
        type="text"
        class="trust-input"
        placeholder="Email or domain (e.g. @github.com)"
        aria-label="Add trusted email or domain"
        bind:value={newTrust}
        oninput={() => { trustError = ""; }} />
      <button type="submit" class="btn-add" disabled={!newTrust.trim()}>Add</button>
    </form>
    {#each ui.trustedSenders as entry (entry)}
      <div class="row trusted">
        <span class="label addr">{entry}</span>
        <button type="button" class="icon-btn sm" title={`Stop always loading images from ${entry}`} aria-label={`Remove ${entry}`} onclick={() => untrustSender(entry)}>
          <Icon name="x" size={14} />
        </button>
      </div>
    {/each}
  </div>
  {#if trustError}
    <p class="hint err">{trustError}</p>
  {:else}
    <p class="hint">Images from these senders and domains (including subdomains) load automatically without asking.</p>
  {/if}
</div>

<style>
  .gname { font-size: 12px; font-weight: 600; color: var(--muted); margin-bottom: 8px; }
  .subname { font-size: 11.5px; font-weight: 600; color: var(--muted); margin: 14px 0 7px; }
  .group + .group { margin-top: 24px; }
  .acc-bar { width: 3px; height: 24px; border-radius: 2px; flex: none; }
  .acc-alert { gap: 10px; }
  .notif-toggle { display: inline-flex; align-items: center; gap: 6px; flex: none; }
  .notif-lbl { font-size: 12px; color: var(--muted); }
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
  .row.disabled { opacity: 0.55; }
  .row.disabled:hover { background: transparent; }
  .row.trusted { padding: 7px 8px 7px 14px; }
  .row.add-trusted { padding: 8px 8px 8px 14px; gap: 8px; }
  .trust-input {
    flex: 1;
    min-width: 0;
    border: 0;
    background: transparent;
    color: var(--text);
    font-size: 13.5px;
    outline: none;
  }
  .trust-input::placeholder { color: var(--muted); }
  .btn-add {
    flex: none;
    height: 28px;
    padding: 0 12px;
    border: 1px solid var(--line-strong);
    border-radius: 6px;
    background: var(--bg);
    color: var(--text);
    font-size: 12.5px;
    font-weight: 500;
  }
  .btn-add:hover:not(:disabled) { background: var(--hover); }
  .btn-add:disabled { opacity: 0.45; }
  input[type="radio"] { margin: 0; accent-color: var(--accent); flex: none; }
  .text { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
  .label { font-size: 13.5px; color: var(--text); }
  .addr { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .detail { font-size: 12.5px; color: var(--muted); }
  .hint { margin: 8px 0 0; font-size: 12px; color: var(--muted); }
  .hint.err { color: var(--danger); }
  .icon-btn.sm { width: 26px; height: 26px; flex: none; }
</style>
