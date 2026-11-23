<script lang="ts">
  import { onMount } from "svelte";
  import { ask } from "@tauri-apps/plugin-dialog";
  import { api, type AgentAccess } from "../api";

  // Access for coding agents on this Mac, through the MCP server in the app (crates/mail-mcp).
  let access = $state<AgentAccess | null>(null);
  let busy = $state(false);
  let failed = $state<string | null>(null);
  let copied = $state<string | null>(null);

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
  /**
   * One-line setup for each agent, in the agents' own commands. Each can be run again:
   * it replaces the earlier entry, which is how a new token reaches an agent after a reset.
   * Codex has no flag for a header, so its entry goes straight into its config file.
   */
  const clients = $derived(
    access && token
      ? [
          {
            name: "Claude Code",
            command: `claude mcp remove mailvern -s user >/dev/null 2>&1; claude mcp add --transport http --scope user mailvern ${access.url} --header "Authorization: Bearer ${token}"`,
          },
          {
            name: "Codex",
            command: `codex mcp remove mailvern >/dev/null 2>&1; printf '\\n[mcp_servers.mailvern]\\nurl = "${access.url}"\\nhttp_headers = { Authorization = "Bearer ${token}" }\\n' >> ~/.codex/config.toml`,
          },
          { name: "Opencode", command: `opencode mcp add --global mailvern --url ${access.url} --header "Authorization=Bearer ${token}"` },
        ]
      : [],
  );
  /** The token stays off the screen; Copy gives the real thing. */
  const masked = (s: string) => s.replaceAll(token, "••••••••");

  async function copy(text: string, what: string) {
    try {
      await api.copyText(text);
      copied = what;
      setTimeout(() => copied === what && (copied = null), 1600);
    } catch (e) {
      failed = String(e);
    }
  }

  async function reset() {
    const ok = await ask("Agents you have set up will stop working until you run their setup command again.", {
      title: "Reset the access token?",
      kind: "warning",
      okLabel: "Reset",
      cancelLabel: "Cancel",
    });
    if (ok) change(api.resetAgentToken());
  }
</script>

<div class="group">
  <div class="gname">Coding agents</div>
  <label class="row top">
    <input type="checkbox" checked={access?.enabled ?? false} disabled={busy || !access} onchange={(e) => change(api.setAgentAccess(e.currentTarget.checked))} />
    <span class="text">
      <span class="label">Let agents on this Mac use Mailvern</span>
      <span class="detail">
        Coding agents such as Claude Code, Codex and Opencode can then search and read your mail, open attachments and save drafts.
        They can't send, move or delete anything.
      </span>
      {#if failed}
        <span class="detail err">{failed}</span>
      {:else if access?.error}
        <span class="detail err">Couldn't start: {access.error}</span>
      {:else if access?.running}
        <span class="detail on">On, at {access.url}, while Mailvern is open.</span>
      {/if}
    </span>
  </label>
</div>

{#if access?.enabled && token}
  <div class="group">
    <div class="gname">Set up an agent</div>
    {#each clients as c (c.name)}
      <div class="row">
        <span class="text">
          <span class="label">{c.name}</span>
          <span class="detail">Run this in Terminal. After a token reset, run it again.</span>
          <code>{masked(c.command)}</code>
        </span>
        <button class="btn" onclick={() => copy(c.command, c.name)}>{copied === c.name ? "Copied" : "Copy"}</button>
      </div>
    {/each}
    <div class="row">
      <span class="text">
        <span class="label">Another agent</span>
        <span class="detail">Add an MCP server of type HTTP with this address, and this header.</span>
        <code>{access.url}</code>
        <code>Authorization: Bearer {masked(token)}</code>
      </span>
      <span class="stack">
        <button class="btn" onclick={() => copy(access!.url, "address")}>{copied === "address" ? "Copied" : "Copy address"}</button>
        <button class="btn" onclick={() => copy(`Authorization: Bearer ${token}`, "header")}>{copied === "header" ? "Copied" : "Copy header"}</button>
      </span>
    </div>
  </div>

  <div class="group">
    <div class="gname">Access token</div>
    <div class="row">
      <span class="text">
        <span class="label">Reset the token</span>
        <span class="detail">The token in the commands above is what lets an agent in. Reset it if someone else may have seen it.</span>
      </span>
      <button class="btn danger" disabled={busy} onclick={reset}>Reset</button>
    </div>
  </div>
{/if}

<p class="note">
  Mail is written by other people, and an agent that reads a message can be misled by what it says.
  That is why agents can only save drafts here: you read each one and send it yourself.
  The mail an agent reads goes to the company that runs its model.
</p>

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
  .row.top { align-items: flex-start; }
  .row.top input { margin: 2px 0 0; accent-color: var(--accent); }
  .text { flex: 1; display: flex; flex-direction: column; gap: 2px; min-width: 0; }
  .stack { display: flex; flex-direction: column; gap: 6px; flex: none; }
  .group + .group { margin-top: 22px; }
  .label { font-size: 13.5px; }
  .detail { font-size: 12.5px; color: var(--muted); }
  .detail.err { color: var(--danger); }
  .detail.on { color: var(--text-2); }
  code {
    margin-top: 4px;
    padding: 6px 8px;
    border-radius: 6px;
    background: var(--hover);
    font: 11.5px/1.5 ui-monospace, "SF Mono", Menlo, monospace;
    color: var(--text-2);
    overflow-wrap: anywhere;
    user-select: text;
  }
  .btn { flex: none; }
  .note { margin: 14px 0 0; font-size: 12.5px; line-height: 1.5; color: var(--muted); }
</style>
