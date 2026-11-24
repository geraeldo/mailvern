<script lang="ts">
  import { onMount, tick } from "svelte";
  import { ask } from "@tauri-apps/plugin-dialog";
  import { api, type AgentAccess } from "../api";
  import Icon from "./Icon.svelte";
  import Switch from "./Switch.svelte";

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

  const token = $derived(access?.token && /^[A-Za-z0-9_-]+$/.test(access.token) ? access.token : "");
  const header = $derived(`Authorization: Bearer ${token}`);
  const shQuote = (s: string) => `'${s.replaceAll("'", "'\\''")}'`;
  const commands = $derived<Record<string, string>>(
    access && token
      ? {
          "Claude Code": `claude mcp remove mailvern -s user >/dev/null 2>&1; claude mcp add --transport http --scope user mailvern ${shQuote(access.url)} --header ${shQuote(`Authorization: Bearer ${token}`)}`,
          Codex: `codex mcp remove mailvern >/dev/null 2>&1; mkdir -p ~/.codex && printf '\\n[mcp_servers.mailvern]\\nurl = "%s"\\nhttp_headers = { Authorization = "Bearer %s" }\\n' ${shQuote(access.url)} ${shQuote(token)} >> ~/.codex/config.toml`,
          OpenCode: `opencode mcp add --global mailvern --url ${shQuote(access.url)} --header ${shQuote(`Authorization=Bearer ${token}`)}`,
        }
      : {},
  );
  const tabs = ["Claude Code", "Codex", "OpenCode", "Other"];
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
    const ok = await ask("Connected agents stop working until you rerun their command.", {
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
  <div class="toggle-card">
    <label class="toggle">
      <span class="text">
        <span class="label">
          Let agents on this Mac use Mailvern
          {#if access?.running}<span class="live"><span class="live-dot"></span>Running</span>{/if}
        </span>
        <span class="detail">Read mail, save drafts, and propose rules. Nothing sends, moves or deletes without your approval.</span>
      </span>
      <Switch
        checked={access?.enabled ?? false}
        disabled={busy || !access}
        label="Let agents on this Mac use Mailvern"
        onchange={(on) => change(api.setAgentAccess(on))} />
    </label>
  </div>
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
          <span>New token. Rerun the command for each agent.</span>
          <button type="button" class="btn ghost sm" onclick={() => (wasReset = false)}>Done</button>
        </div>
      {/if}

      <div class="seg" role="tablist">
        {#each tabs as t (t)}
          <button type="button" role="tab" aria-selected={tab === t} class:on={tab === t} onclick={() => (tab = t)}>
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
          <button type="button" class="icon-btn" title="Copy URL" onclick={() => copy(access!.url, "url", "Other")}>
            <Icon name={copied === "url" ? "check" : "copy"} size={14} />
          </button>
        </div>
        <div class="field-row">
          <span class="key">Header</span>
          <code>{masked(header)}</code>
          <button type="button" class="icon-btn" title="Copy header" onclick={() => copy(header, "header", "Other")}>
            <Icon name={copied === "header" ? "check" : "copy"} size={14} />
          </button>
        </div>
        <p class="hint">Add an HTTP MCP server with this URL and header.</p>
      {:else}
        <div class="cmd">
          <code>{masked(commands[tab])}</code>
          <button type="button" class="btn sm" onclick={() => copy(commands[tab], tab)}>
            <Icon name={copied === tab ? "check" : "copy"} size={13} />
            {copied === tab ? "Copied" : "Copy"}
          </button>
        </div>
        <p class="hint">Paste into Terminal and run.</p>
      {/if}

      <div class="foot">
        <span class="detail">Access token <code class="inline">{masked(token)}</code></span>
        <button type="button" class="btn ghost sm danger" disabled={busy} onclick={reset}>Reset token</button>
      </div>
    </div>
  </section>
{/if}

<p class="note">
  Drafts always wait for you to send. What an agent reads goes to its model provider.
</p>

<style>
  section + section { margin-top: 24px; }
  h3 { margin: 0 0 8px; font-size: 12px; font-weight: 600; color: var(--muted); }

  .toggle-card {
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--panel);
    overflow: hidden;
  }
  .toggle { display: flex; align-items: center; gap: 16px; padding: 12px 14px; cursor: default; }
  .toggle:hover { background: var(--hover); }
  .text { flex: 1; display: flex; flex-direction: column; gap: 2px; min-width: 0; }
  .label { display: flex; align-items: center; flex-wrap: wrap; gap: 8px; font-size: 13.5px; color: var(--text); }
  .live {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 1px 7px;
    border-radius: 99px;
    background: color-mix(in srgb, var(--success) 14%, transparent);
    color: var(--success);
    font-size: 11px;
    font-weight: 600;
  }
  .live-dot { width: 6px; height: 6px; border-radius: 50%; background: var(--success); }
  .detail { font-size: 12.5px; color: var(--muted); }
  .err { margin: 8px 0 0; font-size: 12.5px; color: var(--danger); }

  .card {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 12px;
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--panel);
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
  .seg button:hover:not(.on) { color: var(--text); }
  .seg button.on { background: var(--panel); color: var(--text); box-shadow: 0 1px 2px rgba(0, 0, 0, 0.08); }
  .mark { display: inline-grid; place-items: center; }
  .mark.due { width: 6px; height: 6px; border-radius: 50%; background: var(--accent); }
  .mark.ok { color: var(--accent); }

  code {
    font: 11.5px/1.55 ui-monospace, "SF Mono", Menlo, monospace;
    color: var(--text-2);
    overflow-wrap: anywhere;
    user-select: text;
    -webkit-user-select: text;
  }
  .cmd {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 8px 8px 8px 10px;
    border-radius: 8px;
    background: var(--hover);
  }
  .cmd code { flex: 1; min-width: 0; padding-top: 4px; }
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

  .note { margin: 16px 0 0; font-size: 12px; line-height: 1.5; color: var(--muted); }
</style>
