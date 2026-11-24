<script lang="ts">
  import { ask } from "@tauri-apps/plugin-dialog";
  import Icon from "./Icon.svelte";
  import Modal from "./Modal.svelte";
  import Shortcuts from "./Shortcuts.svelte";
  import General from "./General.svelte";
  import AISettings from "./AISettings.svelte";
  import AgentSettings from "./AgentSettings.svelte";
  import RulesSettings from "./RulesSettings.svelte";
  import AccountSettings from "./AccountSettings.svelte";
  import { rulesUi, isRuleDraftDirty } from "../rules.svelte";
  import { ui, accountLabel, type SettingsSection } from "../state.svelte";

  const sections: { id: Exclude<SettingsSection, "accounts">; label: string; icon: string }[] = [
    { id: "general", label: "General", icon: "settings" },
    { id: "rules", label: "Rules", icon: "filter" },
    { id: "ai", label: "Models", icon: "sparkle" },
    { id: "agents", label: "Agent access", icon: "code" },
    { id: "shortcuts", label: "Shortcuts", icon: "keyboard" },
  ];

  let accountPane = $state<AccountSettings>();

  const section = $derived<SettingsSection>(ui.settings ?? "general");
  const firstRun = $derived(ui.accounts.length === 0 && section === "accounts");
  const activeAccount = $derived(
    typeof ui.accountTarget === "number" ? ui.accounts.find((a) => a.id === ui.accountTarget) : null,
  );
  const title = $derived.by(() => {
    if (section === "rules" && rulesUi.draft) return rulesUi.draft.id ? "Edit rule" : "New rule";
    if (section === "accounts") {
      if (firstRun) return "Add your first account";
      return activeAccount ? accountLabel(activeAccount) : "New account";
    }
    return sections.find((s) => s.id === section)?.label ?? "Settings";
  });

  async function confirmDiscard(): Promise<boolean> {
    if (section === "rules" && isRuleDraftDirty()) {
      return ask("Discard unsaved changes to this rule?", {
        title: "Discard rule changes?",
        kind: "warning",
        okLabel: "Discard",
        cancelLabel: "Keep editing",
      });
    }
    if (section === "accounts" && accountPane?.isDirty()) {
      return ask("Discard unsaved changes to this account?", {
        title: "Discard account changes?",
        kind: "warning",
        okLabel: "Discard",
        cancelLabel: "Keep editing",
      });
    }
    return true;
  }

  async function leaveRuleEditor() {
    if (!(await confirmDiscard())) return;
    rulesUi.draft = null;
    rulesUi.draftOrigin = "";
  }

  async function switchSection(next: SettingsSection, accountTarget?: number | "new") {
    if (next === section && (next !== "accounts" || accountTarget === ui.accountTarget)) return;
    if (!(await confirmDiscard())) return;
    rulesUi.draft = null;
    rulesUi.draftOrigin = "";
    if (next === "accounts") ui.accountTarget = accountTarget ?? ui.accounts[0]?.id ?? "new";
    ui.settings = next;
  }

  async function close() {
    if (!(await confirmDiscard())) return;
    ui.settings = null;
    rulesUi.draft = null;
    rulesUi.draftOrigin = "";
    rulesUi.source = null;
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape" && !e.defaultPrevented && section === "rules" && rulesUi.draft) {
      e.preventDefault();
      leaveRuleEditor();
    }
  }
</script>

<Modal label="Settings" onclose={close} onkeydown={onKey} medium={firstRun}>
  {#if !firstRun}
    <aside class="scroll">
      <div class="side-title">Settings</div>
      {#each sections as s (s.id)}
        <button type="button" class="nav" class:active={section === s.id} onclick={() => switchSection(s.id)}>
          <Icon name={s.icon} size={16} />
          <span class="nav-lbl">{s.label}</span>
          {#if s.id === "rules" && rulesUi.rules.some((r) => r.needsApproval)}
            <span class="pill-warn" title="Rules waiting for approval"></span>
          {/if}
        </button>
      {/each}

      <div class="side-sep"></div>
      <div class="side-title">Accounts</div>
      {#each ui.accounts as a (a.id)}
        <button
          type="button"
          class="nav acc"
          class:active={section === "accounts" && ui.accountTarget === a.id}
          onclick={() => switchSection("accounts", a.id)}>
          <span class="acc-bar" style:background={a.color || "var(--muted)"}></span>
          <span class="acc-text">
            <span class="acc-name">{accountLabel(a)}</span>
            <span class="acc-mail">{a.email}</span>
          </span>
          {#if ui.status[a.id]?.state === "error"}
            <span class="warn" title={ui.status[a.id].message ?? "Connection error"}><Icon name="alert" size={13} /></span>
          {/if}
        </button>
      {/each}
      <button
        type="button"
        class="nav add"
        class:active={section === "accounts" && ui.accountTarget === "new"}
        onclick={() => switchSection("accounts", "new")}>
        <Icon name="plus" size={15} /> Add account
      </button>
    </aside>
  {/if}

  <div class="pane">
    <header>
      <div class="title-wrap">
        {#if section === "rules" && rulesUi.draft}
          <button type="button" class="back-btn" title="Back to Rules  Esc" onclick={leaveRuleEditor}>
            <Icon name="arrowLeft" size={14} /> Rules
          </button>
          <span class="crumb">/</span>
        {/if}
        <h2>{title}</h2>
      </div>
      <button type="button" class="icon-btn" title="Close  Esc" onclick={close}><Icon name="x" /></button>
    </header>

    <main class="scroll">
      {#if section === "general"}
        <General />
      {:else if section === "accounts"}
        <AccountSettings
          bind:this={accountPane}
          target={ui.accountTarget}
          {firstRun}
          onselect={(id) => (ui.accountTarget = id)}
          onclose={close} />
      {:else if section === "rules"}
        <RulesSettings onback={leaveRuleEditor} />
      {:else if section === "ai"}
        <AISettings />
      {:else if section === "agents"}
        <AgentSettings />
      {:else if section === "shortcuts"}
        <Shortcuts />
      {/if}
    </main>
  </div>
</Modal>

<style>
  aside {
    width: 236px;
    flex: none;
    display: flex;
    flex-direction: column;
    padding: 18px 10px 14px;
    background: var(--side);
    border-right: 1px solid var(--line);
  }
  .side-title { font-size: 11.5px; font-weight: 600; color: var(--muted); padding: 0 10px 8px; letter-spacing: 0.01em; }
  .side-sep { height: 1px; background: var(--line); margin: 14px 10px; flex: none; }
  .nav {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 7px 10px;
    border: 0;
    border-radius: 8px;
    background: transparent;
    text-align: left;
    font-size: 13.5px;
    color: var(--text-2);
  }
  .nav:hover { background: var(--hover); color: var(--text); }
  .nav.active { background: var(--active); color: var(--text); font-weight: 500; }
  .nav :global(svg) { color: var(--muted); flex: none; }
  .nav.active :global(svg) { color: var(--text); }
  .nav-lbl { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .pill-warn { width: 7px; height: 7px; border-radius: 50%; background: var(--warn); flex: none; }

  .acc { padding: 7px 10px; }
  .acc-bar { width: 3px; height: 22px; border-radius: 2px; flex: none; }
  .acc-text { display: flex; flex-direction: column; min-width: 0; flex: 1; }
  .acc-name { font-size: 13px; font-weight: 500; color: var(--text); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .acc-mail { font-size: 11.5px; color: var(--muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .nav.add { color: var(--accent); font-weight: 500; margin-top: 4px; }
  .nav.add :global(svg) { color: var(--accent); }
  .warn { color: var(--danger); display: grid; flex: none; }

  .pane {
    flex: 1;
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    height: 58px;
    flex: none;
    padding: 0 18px 0 28px;
    border-bottom: 1px solid var(--line);
  }
  .title-wrap { display: flex; align-items: center; gap: 8px; min-width: 0; }
  .back-btn {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    height: 26px;
    padding: 0 8px 0 6px;
    margin-left: -6px;
    border: 0;
    border-radius: 6px;
    background: transparent;
    color: var(--muted);
    font-size: 13.5px;
    font-weight: 500;
    flex: none;
  }
  .back-btn:hover { background: var(--hover); color: var(--text); }
  .crumb { color: var(--line-strong); font-size: 14px; flex: none; }
  h2 {
    margin: 0;
    font-size: 17px;
    font-weight: 650;
    letter-spacing: -0.01em;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  main { flex: 1; min-height: 0; padding: 20px 28px 28px; }
</style>
