<script lang="ts">
  import { onMount } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { isPermissionGranted, requestPermission, sendNotification } from "@tauri-apps/plugin-notification";
  import Sidebar from "./lib/components/Sidebar.svelte";
  import MessageList from "./lib/components/MessageList.svelte";
  import Reader from "./lib/components/Reader.svelte";
  import Composer from "./lib/components/Composer.svelte";
  import Settings from "./lib/components/Settings.svelte";
  import AccountSettings from "./lib/components/AccountSettings.svelte";
  import { api, type MailEvent } from "./lib/api";
  import { allowNativeMenu } from "./lib/menus";
  import {
    ui, loadAccounts, loadFolders, loadList, loadTrusted, move, compose,
    archiveSelected, trashSelected, toggleJunk, toggleSeen, toggleFlag, accountById, openSettings, openAccountSettings,
  } from "./lib/state.svelte";
  import { keymap, comboOf, match, canFireWhileTyping, usesModifier, loadShortcuts, type Action } from "./lib/shortcuts.svelte";

  let list: MessageList;
  let notify = false;

  async function refreshBadge() {
    try {
      const n = await api.unreadCount();
      await getCurrentWindow().setBadgeCount(n > 0 ? n : undefined);
    } catch {}
  }

  let pending: ReturnType<typeof setTimeout> | undefined;
  function scheduleRefresh() {
    clearTimeout(pending);
    pending = setTimeout(async () => {
      await Promise.all([loadFolders(), loadList(true)]);
      refreshBadge();
    }, 150);
  }

  onMount(() => {
    (async () => {
      await loadAccounts();
      await Promise.all([loadFolders(), loadList(true), loadTrusted(), loadShortcuts()]);
      refreshBadge();
      if (!ui.accounts.length) openAccountSettings("new");
      notify = (await isPermissionGranted()) || (await requestPermission()) === "granted";
    })();

    const un = listen<MailEvent>("mail-event", ({ payload: e }) => {
      if (e.kind === "changed") scheduleRefresh();
      else if (e.kind === "status") ui.status[e.accountId] = { state: e.state, message: e.message };
      else if (e.kind === "newMail" && notify) {
        const acc = accountById(e.accountId);
        sendNotification({
          title: e.count > 1 ? `${e.count} new messages` : e.from,
          body: e.count > 1 ? `${e.from} · ${e.subject}` : e.subject || "(no subject)",
          ...(acc && ui.accounts.length > 1 ? { subtitle: acc.email } : {}),
        } as any);
      }
    });
    return () => {
      un.then((f) => f());
    };
  });

  function typing(e: KeyboardEvent) {
    const t = e.target as HTMLElement;
    return t.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(t.tagName);
  }

  const run: Record<Action, () => void> = {
    next: () => move(1),
    prev: () => move(-1),
    reply: () => compose("reply"),
    replyAll: () => compose("replyAll"),
    forward: () => compose("forward"),
    compose: () => compose(),
    archive: archiveSelected,
    delete: trashSelected,
    junk: toggleJunk,
    toggleUnread: toggleSeen,
    toggleFlag: toggleFlag,
    search: () => list?.focusSearch(),
    sync: () => api.syncNow(),
    settings: () => openSettings(),
    send: () => {},
  };

  function onKey(e: KeyboardEvent) {
    if (keymap.recording) return;
    const combo = comboOf(e);
    if (!combo) return;
    const action = match(combo);
    // Composer and the settings popups handle their own keys.
    if (!action || action === "send" || ui.compose || ui.settings || ui.accountSettings) return;
    if (typing(e) && !(canFireWhileTyping(action) && usesModifier(combo))) return;
    e.preventDefault();
    run[action]();
  }
</script>

<svelte:window onkeydown={onKey} oncontextmenu={(e) => { if (!allowNativeMenu(e)) e.preventDefault(); }} />

<div class="app">
  <Sidebar />
  <MessageList bind:this={list} />
  <Reader />
</div>

{#if ui.compose}
  {#key ui.compose.key}
    <Composer init={ui.compose} />
  {/key}
{/if}

{#if ui.settings}
  <Settings />
{/if}

{#if ui.accountSettings}
  <AccountSettings />
{/if}

{#if ui.toast}
  <div class="toast" role="status">{ui.toast.text}</div>
{/if}

<style>
  .app {
    display: grid;
    grid-template-columns: 236px minmax(320px, 390px) minmax(0, 1fr);
    /* One row exactly the window's height, so each column scrolls on its own. */
    grid-template-rows: minmax(0, 100%);
    height: 100%;
  }
  .app > :global(*) { min-height: 0; }
  .toast {
    position: fixed;
    left: 50%;
    bottom: 22px;
    transform: translateX(-50%);
    z-index: 100;
    max-width: min(560px, calc(100vw - 48px));
    padding: 9px 16px;
    border-radius: 10px;
    background: #1d1c1a;
    color: #fff;
    font-size: 13px;
    font-weight: 500;
    box-shadow: var(--shadow);
    animation: rise 0.16s ease-out;
    -webkit-user-select: text;
    user-select: text;
  }
  @media (prefers-color-scheme: dark) {
    .toast { background: #3a3936; }
  }
</style>
