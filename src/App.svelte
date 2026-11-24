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
  import ShortcutHelp from "./lib/components/ShortcutHelp.svelte";
  import WhenPicker from "./lib/components/WhenPicker.svelte";
  import MovePicker from "./lib/components/MovePicker.svelte";
  import { api, type MailEvent } from "./lib/api";
  import { allowNativeMenu } from "./lib/menus";
  import { refreshBadge, loadBadgeMode } from "./lib/badge.svelte";
  import { loadTheme } from "./lib/theme.svelte";
  import { loadAi } from "./lib/ai/config.svelte";
  import {
    ui, loadAccounts, loadFolders, loadList, loadTrusted, loadPrefs, loadOutbox, loadPendingAgentActions, refreshThread, move, compose,
    archiveSelected, trashSelected, toggleJunk, toggleSeen, toggleFlag, accountById, openSettings, openAccountSettings,
    moveSelected, snoozeSelected, unsnoozeSelected, selectAll, extendSelection, clearSelection, snoozeRows, moveRows,
    selectedRows, roleOf, toast, undoToast, runToastAction, copyThread, openMessage, commitPending, setPaneWidths, toggleSidebar,
  } from "./lib/state.svelte";
  import { snoozePresets } from "./lib/when";
  import { exitAi } from "./lib/ai/search.svelte";
  import { loadRules, rulesUi } from "./lib/rules.svelte";
  import { keymap, comboOf, match, canFireWhileTyping, usesModifier, loadShortcuts, type Action } from "./lib/shortcuts.svelte";

  let list: MessageList;
  let notify = false;
  let windowFocused = true;
  /** Rules waiting for approval, to say so when an agent adds one. */
  let pendingRules = 0;
  let resizing = $state<"side" | "list" | null>(null);

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
    loadTheme();
    (async () => {
      await loadAccounts();
      await loadPrefs();
      await Promise.all([
        loadFolders(), loadList(true), loadTrusted(), loadShortcuts(), loadBadgeMode(), loadOutbox(), loadAi(), loadPendingAgentActions(),
      ]);
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
      } else if (e.kind === "rulePaused") {
        toast(`Paused the rule “${e.name}”: ${e.reason}. Turn it on again in Settings › Rules.`, { ms: 8000 });
        loadRules();
      } else if (e.kind === "rulesChanged") {
        if (rulesUi.loaded) loadRules();
        if (e.pendingApproval > pendingRules) toast("An agent added or changed a rule. Review it in Settings › Rules.", { ms: 8000 });
        pendingRules = e.pendingApproval;
      } else if (e.kind === "agentActionsChanged") {
        const prev = ui.pendingAgentActions;
        ui.pendingAgentActions = e.pending;
        if (e.pending > prev) {
          const msg = e.pending === 1 ? "An agent action is waiting for your approval." : `${e.pending} agent actions are waiting for your approval.`;
          toast(msg, { ms: 8000, action: { label: "Review", fn: () => openSettings("agents") } });
          if (notify && !windowFocused) sendNotification({ title: "Agent waiting for approval", body: msg });
        }
      } else if (e.kind === "agentSent") {
        loadOutbox();
        const who = e.to[0] ? ` to ${e.to[0].replace(/\s*<.*>$/, "")}` : "";
        const subj = e.subject || "(no subject)";
        const undoMs = Math.max(4000, (ui.undoSend || 5) * 1000);
        toast(`Agent sending “${subj}”${who}`, {
          ms: undoMs,
          undo: ui.undoSend > 0
            ? async () => {
                try {
                  await api.cancelSend(e.outboxId);
                  loadOutbox();
                  toast("Send cancelled");
                } catch (err) {
                  toast(String(err));
                }
              }
            : undefined,
        });
      } else if (e.kind === "sendFailed") {
        toast(`Couldn't send “${e.subject || "(no subject)"}”. It will retry from Scheduled.`, { ms: 6000 });
        if (notify) sendNotification({ title: "Message not sent", body: `${e.subject || "(no subject)"}: ${e.message}` });
      }
    });
    // Bringing the window forward counts as looking at your mail: clear "new" from the badge.
    const unFocus = getCurrentWindow().onFocusChanged(({ payload: focused }) => {
      windowFocused = focused;
      if (focused) refreshBadge();
    });
    const onUnload = () => commitPending();
    window.addEventListener("beforeunload", onUnload);
    return () => {
      commitPending();
      window.removeEventListener("beforeunload", onUnload);
      un.then((f) => f());
      unFocus.then((f) => f());
    };
  });

  function startResize(which: "side" | "list", e: PointerEvent) {
    e.preventDefault();
    resizing = which;
    const startX = e.clientX;
    const startSide = ui.sideWidth;
    const startList = ui.listWidth;
    document.body.style.cursor = "col-resize";

    const onMove = (ev: PointerEvent) => {
      const dx = ev.clientX - startX;
      if (which === "side") {
        ui.sideWidth = Math.round(Math.max(170, Math.min(320, startSide + dx)));
      } else {
        const maxList = Math.min(480, Math.max(260, window.innerWidth - (ui.sidebarCollapsed ? 0 : ui.sideWidth) - 360));
        ui.listWidth = Math.round(Math.max(260, Math.min(maxList, startList + dx)));
      }
    };
    const onUp = () => {
      resizing = null;
      document.body.style.cursor = "";
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onUp);
      setPaneWidths(ui.sideWidth, ui.listWidth);
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onUp);
  }

  function resetPane(which: "side" | "list") {
    if (which === "side") setPaneWidths(214, ui.listWidth);
    else setPaneWidths(ui.sideWidth, 336);
  }

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
    // Composer, pickers, shortcut guide and the settings popup handle their own keys.
    if (ui.compose || ui.settings || ui.picker || ui.shortcutHelp) return;
    if (combo === "mod+alt+s") {
      e.preventDefault();
      toggleSidebar();
      return;
    }
    if (combo === "mod+z" && !typing(e) && ui.toast?.undo) {
      e.preventDefault();
      undoToast();
      return;
    }
    if (e.key === "?" && !e.metaKey && !e.ctrlKey && !e.altKey && !typing(e)) {
      e.preventDefault();
      ui.shortcutHelp = true;
      return;
    }
    if (combo === "escape" && !typing(e) && clearSelection()) {
      e.preventDefault();
      return;
    }
    // AI answer: Escape goes back from a source to the answer, then out of the answer.
    if (combo === "escape" && !typing(e) && ui.ai) {
      e.preventDefault();
      if (ui.selectedId != null) openMessage(null);
      else exitAi();
      return;
    }
    const action = match(combo);
    if (!action || action === "send") return;
    if (typing(e) && !(canFireWhileTyping(action) && usesModifier(combo))) return;
    e.preventDefault();
    run[action]();
  }

  const gridCols = $derived(
    ui.sidebarCollapsed
      ? `minmax(260px, ${ui.listWidth}px) minmax(0, 1fr)`
      : `minmax(170px, ${ui.sideWidth}px) minmax(260px, ${ui.listWidth}px) minmax(0, 1fr)`,
  );
</script>

<svelte:window onkeydown={onKey} oncontextmenu={(e) => { if (!allowNativeMenu(e)) e.preventDefault(); }} />

<div class="app" class:resizing={!!resizing} style:grid-template-columns={gridCols}>
  {#if !ui.sidebarCollapsed}
    <div class="pane">
      <Sidebar />
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="splitter"
        class:active={resizing === "side"}
        title="Drag to resize sidebar (double-click to reset)"
        onpointerdown={(e) => startResize("side", e)}
        ondblclick={() => resetPane("side")}></div>
    </div>
  {/if}
  <div class="pane">
    <MessageList bind:this={list} />
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="splitter"
      class:active={resizing === "list"}
      title="Drag to resize list (double-click to reset)"
      onpointerdown={(e) => startResize("list", e)}
      ondblclick={() => resetPane("list")}></div>
  </div>
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

{#if ui.shortcutHelp}
  <ShortcutHelp onclose={() => (ui.shortcutHelp = false)} />
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
      <button type="button" class="undo" onclick={undoToast}>Undo</button>
    {:else if ui.toast.action}
      <button type="button" class="undo" onclick={runToastAction}>{ui.toast.action.label}</button>
    {/if}
  </div>
{/if}

<style>
  .app {
    display: grid;
    /* One row exactly the window's height, so each column scrolls on its own. */
    grid-template-rows: minmax(0, 100%);
    height: 100%;
  }
  .app > :global(*) { min-height: 0; }
  .app.resizing :global(iframe) { pointer-events: none; }
  .pane { position: relative; min-width: 0; min-height: 0; height: 100%; }
  .splitter {
    position: absolute;
    top: 0;
    right: -3px;
    bottom: 0;
    width: 6px;
    z-index: 15;
    cursor: col-resize;
  }
  .splitter::after {
    content: "";
    position: absolute;
    top: 0;
    bottom: 0;
    left: 2px;
    width: 2px;
    background: transparent;
    transition: background 0.12s;
  }
  .splitter:hover::after, .splitter.active::after { background: var(--accent); opacity: 0.55; }
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
    padding: 2px 6px;
    margin: -2px -4px -2px 0;
    border-radius: 5px;
    background: none;
    color: #8fb4ff;
    font: inherit;
    font-weight: 600;
  }
  .undo:hover { background: rgba(143, 180, 255, 0.14); }
  @media (prefers-color-scheme: dark) {
    .toast { background: #2c2b29; border: 1px solid var(--line-strong); }
  }
</style>
