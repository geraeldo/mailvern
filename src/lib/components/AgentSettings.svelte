<script lang="ts">
  import { onMount, tick } from "svelte";
  import { ask } from "@tauri-apps/plugin-dialog";
  import { api, type AgentAccess } from "../api";
  import Icon from "./Icon.svelte";

  // Access for coding agents on this Mac, through the MCP server in the app (crates/mail-mcp).
  let access = $state<AgentAccess | null>(null);
  let busy = $state(false);
  let failed = $state<string | null>(null);
  let copied = $state<string | null>(null);
  let tab = $state("Claude Code");
  /** The token was just reset: every agent set up before is locked out until its command is run again. */
  let wasReset = $state(false);
  /** Agents whose new command was copied since the reset. */
  let redone = $state<string[]>([]);
  let card = $state<HTMLElement>();

  onMount(() => change(api.agentAccess()));

  async function change(next: Promise<AgentAccess>) {
    busy = true;
    failed = null;
    try {
      access = await next;
    } catch (e) {
      failed = String(e);
    }
    busy = false;
  }

  const token = $derived(access?.token ?? "");
  const header = $derived(`Authorization: Bearer ${token}`);
  /**
   * One-line setup for each agent, in the agents' own commands. Each can be run again:
   * it replaces the earlier entry, which is how a new token reaches an agent after a reset.
   * Codex has no flag for a header, so its entry goes straight into its config file.
   */
  const commands = $derived<Record<string, string>>(
    access && token
      ? {
          "Claude Code": `claude mcp remove mailvern -s user >/dev/null 2>&1; claude mcp add --transport http --scope user mailvern ${access.url} --header "Authorization: Bearer ${token}"`,
          Codex: `codex mcp remove mailvern >/dev/null 2>&1; printf '\\n[mcp_servers.mailvern]\\nurl = "${access.url}"\\nhttp_headers = { Authorization = "Bearer ${token}" }\\n' >> ~/.codex/config.toml`,
          Opencode: `opencode mcp add --global mailvern --url ${access.url} --header "Authorization=Bearer ${token}"`,
        }
      : {},
  );
  const tabs = ["Claude Code", "Codex", "Opencode", "Other"];
  /** The token stays off the screen; Copy gives the real thing. */
  const masked = (s: string) => (token ? s.replaceAll(token, "••••••••") : s);

  async function copy(text: string, what: string, agent = what) {
    try {
      await api.copyText(text);
      copied = what;
      if (wasReset && !redone.includes(agent)) redone.push(agent);
      setTimeout(() => copied === what && (copied = null), 1600);
    } catch (e) {
      failed = String(e);
    }
  }

  async function reset() {
    const ok = await ask("Agents you've connected will stop working until you run their command again.", {
      title: "Reset the access token?",
      kind: "warning",
      okLabel: "Reset",
      cancelLabel: "Cancel",
    });
    if (!ok) return;
    await change(api.resetAgentToken());
    if (failed) return;
    wasReset = true;
    redone = [];
    await tick();
    card?.scrollIntoView({ behavior: "smooth", block: "nearest" });
  }
</script>

<section>
  <label class="toggle">
    <span class="text">
      <span class="label">Let agents on this Mac use Mailvern</span>
      <span class="detail">They can read mail and save drafts, never send, move or delete.</span>
    </span>
    <input
      type="checkbox"
      role="switch"
      checked={access?.enabled ?? false}
      disabled={busy || !access}
      onchange={(e) => change(api.setAgentAccess(e.currentTarget.checked))}
    />
  </label>
  {#if failed}
    <p class="err">{failed}</p>
  {:else if access?.enabled && access.error}
    <p class="err">Couldn't start: {access.error}</p>
  {/if}
</section>

{#if access?.enabled && token}
  <section>
    <h3>Connect an agent</h3>
    <div class="card" class:reset={wasReset} bind:this={card}>
      {#if wasReset}
        <div class="banner">
          <Icon name="refresh" size={14} />
          <span>New token. Run the command again for each agent you use.</span>
          <button class="btn ghost sm" onclick={() => (wasReset = false)}>Done</button>
        </div>
      {/if}

      <div class="seg" role="tablist">
        {#each tabs as t (t)}
          <button role="tab" aria-selected={tab === t} class:on={tab === t} onclick={() => (tab = t)}>
            {t}
            {#if wasReset}
              {#if redone.includes(t)}
                <span class="mark ok"><Icon name="check" size={11} stroke={2.5} /></span>
              {:else}
                <span class="mark due"></span>
              {/if}
            {/if}
          </button>
        {/each}
      </div>

      {#if tab === "Other"}
        <div class="field-row">
          <span class="key">URL</span>
          <code>{access.url}</code>
          <button class="icon-btn" title="Copy URL" onclick={() => copy(access!.url, "url", "Other")}>
            <Icon name={copied === "url" ? "check" : "copy"} size={14} />
          </button>
        </div>
        <div class="field-row">
          <span class="key">Header</span>
          <code>{masked(header)}</code>
          <button class="icon-btn" title="Copy header" onclick={() => copy(header, "header", "Other")}>
            <Icon name={copied === "header" ? "check" : "copy"} size={14} />
          </button>
        </div>
        <p class="hint">Add an HTTP MCP server with this URL and header.</p>
      {:else}
        <div class="cmd">
          <code>{masked(commands[tab])}</code>
          <button class="btn sm" onclick={() => copy(commands[tab], tab)}>
            <Icon name={copied === tab ? "check" : "copy"} size={13} />
            {copied === tab ? "Copied" : "Copy"}
          </button>
        </div>
        <p class="hint">Paste into Terminal and run.</p>
      {/if}

      <div class="foot">
        <span class="detail">Access token <code class="inline">{masked(token)}</code></span>
        <button class="btn ghost sm danger" disabled={busy} onclick={reset}>Reset</button>
      </div>
    </div>
  </section>
{/if}

<p class="note">
  Mail can contain text meant to mislead an agent, so drafts always wait for you to send. What an agent reads goes to its model provider.
</p>

<style>
  section + section { margin-top: 24px; }
  h3 { margin: 0 0 8px; font-size: 12px; font-weight: 600; color: var(--muted); }

  .toggle { display: flex; align-items: center; gap: 16px; padding: 4px 0; cursor: default; }
  .text { flex: 1; display: flex; flex-direction: column; gap: 2px; min-width: 0; }
  .label { font-size: 13.5px; }
  .detail { font-size: 12.5px; color: var(--muted); }
  .err { margin: 8px 0 0; font-size: 12.5px; color: var(--danger); }

  /* A switch, drawn on the native checkbox. */
  input[role="switch"] {
    appearance: none;
    flex: none;
    position: relative;
    width: 34px;
    height: 20px;
    margin: 0;
    border-radius: 10px;
    background: var(--line-strong);
    transition: background 0.15s;
    cursor: default;
  }
  input[role="switch"]::after {
    content: "";
    position: absolute;
    top: 2px;
    left: 2px;
    width: 16px;
    height: 16px;
    border-radius: 50%;
    background: #fff;
    box-shadow: 0 1px 2px rgba(0, 0, 0, 0.2);
    transition: transform 0.15s;
  }
  input[role="switch"]:checked { background: var(--accent); }
  input[role="switch"]:checked::after { transform: translateX(14px); }
  input[role="switch"]:disabled { opacity: 0.5; }

  .card {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 12px;
    border: 1px solid var(--line);
    border-radius: var(--radius);
    transition: border-color 0.2s;
  }
  .card.reset { border-color: var(--accent); }

  .banner {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: -4px -4px 0;
    padding: 6px 6px 6px 10px;
    border-radius: 7px;
    background: var(--accent-soft);
    color: var(--accent);
    font-size: 12.5px;
    animation: fade 0.2s;
  }
  .banner span { flex: 1; color: var(--text); }

  .seg { display: flex; align-self: flex-start; padding: 2px; border-radius: 8px; background: var(--hover); }
  .seg button {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    height: 24px;
    padding: 0 10px;
    border: 0;
    border-radius: 6px;
    background: transparent;
    font-size: 12px;
    font-weight: 500;
    color: var(--muted);
  }
  .seg button.on { background: var(--panel); color: var(--text); box-shadow: 0 1px 2px rgba(0, 0, 0, 0.08); }
  .mark { display: inline-grid; place-items: center; }
  .mark.due { width: 6px; height: 6px; border-radius: 50%; background: var(--accent); }
  .mark.ok { color: var(--accent); }

  code {
    font: 11.5px/1.55 ui-monospace, "SF Mono", Menlo, monospace;
    color: var(--text-2);
    overflow-wrap: anywhere;
    user-select: text;
  }
  .cmd {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 8px 8px 8px 10px;
    border-radius: 8px;
    background: var(--hover);
  }
  .cmd code { flex: 1; min-width: 0; padding-top: 3px; }
  .field-row {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 4px 4px 4px 10px;
    border-radius: 8px;
    background: var(--hover);
  }
  .field-row code { flex: 1; min-width: 0; }
  .field-row + .field-row { margin-top: -4px; }
  .key { width: 46px; flex: none; font-size: 11.5px; color: var(--muted); }
  .hint { margin: -2px 0 0; font-size: 12px; color: var(--muted); }

  .btn.sm { height: 26px; padding: 0 10px; font-size: 12px; flex: none; }
  .foot {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-top: 2px;
    padding-top: 10px;
    border-top: 1px solid var(--line);
  }
  .foot .detail { display: flex; align-items: center; gap: 6px; }
  code.inline { letter-spacing: 1px; color: var(--muted); }

  .note { margin: 20px 0 0; font-size: 12px; line-height: 1.5; color: var(--muted); }
</style>
