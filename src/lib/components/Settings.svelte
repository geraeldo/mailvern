<script lang="ts">
  import Icon from "./Icon.svelte";
  import Modal from "./Modal.svelte";
  import Shortcuts from "./Shortcuts.svelte";
  import General from "./General.svelte";
  import AISettings from "./AISettings.svelte";
  import AgentSettings from "./AgentSettings.svelte";
  import { ui, type SettingsSection } from "../state.svelte";

  // App-wide preferences. Account settings live in AccountSettings.
  const sections: { id: SettingsSection; label: string; icon: string }[] = [
    { id: "general", label: "General", icon: "settings" },
    { id: "ai", label: "AI", icon: "sparkle" },
    { id: "agents", label: "Agents", icon: "code" },
    { id: "shortcuts", label: "Shortcuts", icon: "keyboard" },
  ];

  const current = $derived(sections.find((s) => s.id === ui.settings) ?? sections[0]);

  function close() {
    ui.settings = null;
  }
</script>

<Modal label="Settings" onclose={close}>
  <aside>
    <div class="side-title">Settings</div>
    {#each sections as s (s.id)}
      <button class="nav" class:active={current.id === s.id} onclick={() => (ui.settings = s.id)}>
        <Icon name={s.icon} size={16} /> {s.label}
      </button>
    {/each}
  </aside>

  <main class="scroll">
    <header>
      <h2>{current.label}</h2>
      <button class="icon-btn" title="Close  Esc" onclick={close}><Icon name="x" /></button>
    </header>

    {#if current.id === "general"}
      <General />
    {:else if current.id === "ai"}
      <AISettings />
    {:else if current.id === "agents"}
      <AgentSettings />
    {:else if current.id === "shortcuts"}
      <Shortcuts />
    {/if}
  </main>
</Modal>

<style>
  aside {
    width: 240px;
    flex: none;
    display: flex;
    flex-direction: column;
    padding: 18px 10px 12px;
    background: var(--side);
    border-right: 1px solid var(--line);
    overflow-y: auto;
  }
  .side-title { font-size: 12px; font-weight: 600; color: var(--muted); padding: 0 10px 10px; }
  .nav {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 8px 10px;
    border: 0;
    border-radius: 8px;
    background: transparent;
    text-align: left;
    font-size: 13.5px;
    color: var(--text-2);
  }
  .nav:hover { background: var(--hover); }
  .nav.active { background: var(--active); color: var(--text); }
  .nav :global(svg) { color: var(--muted); flex: none; }
  main { flex: 1; display: flex; flex-direction: column; padding: 0 28px 22px; min-width: 0; }
  header { display: flex; align-items: center; justify-content: space-between; height: 64px; flex: none; margin-right: -10px; }
  h2 { margin: 0; font-size: 18px; font-weight: 650; letter-spacing: -0.01em; }
</style>
