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
  import WhenPicker from "./lib/components/WhenPicker.svelte";
  import MovePicker from "./lib/components/MovePicker.svelte";
  import { api, type MailEvent } from "./lib/api";
  import { allowNativeMenu } from "./lib/menus";
  import { refreshBadge, loadBadgeMode } from "./lib/badge.svelte";
  import { loadAi } from "./lib/ai/config.svelte";
  import {
    ui, loadAccounts, loadFolders, loadList, loadTrusted, loadPrefs, loadOutbox, refreshThread, move, compose,
    archiveSelected, trashSelected, toggleJunk, toggleSeen, toggleFlag, accountById, openSettings, openAccountSettings,
    moveSelected, snoozeSelected, unsnoozeSelected, selectAll, extendSelection, clearSelection, snoozeRows, moveRows,
    selectedRows, roleOf, toast, undoToast, copyThread,
  } from "./lib/state.svelte";
  import { snoozePresets } from "./lib/when";
  import { keymap, comboOf, match, canFireWhileTyping, usesModifier, loadShortcuts, type Action } from "./lib/shortcuts.svelte";

  let list: MessageList;
  let notify = false;

  let pending: ReturnType<typeof setTimeout> | undefined;
  function scheduleRefresh() {
    clearTimeout(pending);
    pending = setTimeout(async () => {
      await Promise.all([loadFolders(), loadList(true)]);
      refreshThread();
      refreshBadge();
    }, 150);
  }

  onMount(() => {
    (async () => {
      await loadAccounts();
      await loadPrefs();
      await Promise.all([loadFolders(), loadList(true), loadTrusted(), loadShortcuts(), loadBadgeMode(), loadOutbox(), loadAi()]);
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
      } else if (e.kind === "outbox") {
        loadOutbox();
      } else if (e.kind === "sendFailed") {
        toast(`Couldn't send “${e.subject || "(no subject)"}”. It's in Scheduled and will be retried.`, { ms: 6000 });
        if (notify) sendNotification({ title: "Message not sent", body: `${e.subject || "(no subject)"}: ${e.message}` });
      }
    });
    // Bringing the window forward counts as looking at your mail: clear "new" from the badge.
    const unFocus = getCurrentWindow().onFocusChanged(({ payload: focused }) => {
      if (focused) refreshBadge();
    });
    return () => {
      un.then((f) => f());
      unFocus.then((f) => f());
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
    move: moveSelected,
    snooze: () => (selectedRows().every((r) => roleOf(r) === "snoozed") && selectedRows().length ? unsnoozeSelected() : snoozeSelected()),
    selectAll,
    extendDown: () => extendSelection(1),
    extendUp: () => extendSelection(-1),
    search: () => list?.focusSearch(),
    sync: () => api.syncNow(),
    settings: () => openSettings(),
    send: () => {},
    copyThread,
  };

  function onKey(e: KeyboardEvent) {
    if (keymap.recording) return;
    const combo = comboOf(e);
    if (!combo) return;
    // Composer, pickers and the settings popups handle their own keys.
    if (ui.compose || ui.settings || ui.accountSettings || ui.picker) return;
    if (combo === "escape" && !typing(e) && clearSelection()) {
      e.preventDefault();
      return;
    }
    const action = match(combo);
    if (!action || action === "send") return;
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

{#if ui.picker?.kind === "snooze"}
  <WhenPicker
    title="Snooze until"
    action="Snooze"
    presets={snoozePresets()}
    onpick={(at) => ui.picker?.kind === "snooze" && snoozeRows(ui.picker.rows, at)}
    onclose={() => (ui.picker = null)} />
{:else if ui.picker?.kind === "move"}
  <MovePicker
    accountId={ui.picker.accountId}
    exclude={[...new Set(ui.picker.rows.map((r) => r.folderId))]}
    onpick={(f) => ui.picker?.kind === "move" && moveRows(ui.picker.rows, f)}
    onclose={() => (ui.picker = null)} />
{/if}

{#if ui.toast}
  <div class="toast" role="status">
    <span>{ui.toast.text}</span>
    {#if ui.toast.undo}
      <button class="undo" onclick={undoToast}>Undo</button>
    {/if}
  </div>
{/if}

<style>
  .app {
    display: grid;
    /* Sidebar and list give up width before the reader does, down to the 900px minimum window. */
    grid-template-columns: clamp(188px, 18vw, 236px) clamp(290px, 30vw, 390px) minmax(0, 1fr);
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
    display: flex;
    align-items: center;
    gap: 14px;
  }
  .undo {
    border: 0;
    padding: 0;
    background: none;
    color: #8fb4ff;
    font: inherit;
    font-weight: 600;
  }
  @media (prefers-color-scheme: dark) {
    .toast { background: #3a3936; }
  }
</style>
