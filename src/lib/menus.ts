import { Menu } from "@tauri-apps/api/menu";
import type { Account, Summary } from "./api";
import { api } from "./api";
import {
  ui, openSettings, openMessage, compose, archiveSelected, trashSelected, toggleSeen, toggleFlag,
} from "./state.svelte";

type Item = { text: string; action: () => void; enabled?: boolean } | "-";

async function popup(items: Item[]) {
  const menu = await Menu.new({
    items: items.map((i) =>
      i === "-" ? { item: "Separator" as const } : { text: i.text, enabled: i.enabled ?? true, action: i.action },
    ),
  });
  await menu.popup();
}

export function accountMenu(acc: Account, collapsed: boolean, toggle: () => void) {
  return popup([
    { text: "Account Settings…", action: () => openSettings(acc.id) },
    { text: "Check for Mail", action: () => api.syncNow() },
    "-",
    { text: collapsed ? "Expand" : "Collapse", action: toggle },
  ]);
}

export async function messageMenu(m: Summary) {
  // Act on the message that was right-clicked.
  if (ui.selectedId !== m.id || !ui.detail) await openMessage(m.id);
  const row = ui.list.find((x) => x.id === m.id) ?? m;
  return popup([
    { text: "Reply", action: () => compose("reply"), enabled: !!ui.detail },
    { text: "Reply All", action: () => compose("replyAll"), enabled: !!ui.detail },
    { text: "Forward", action: () => compose("forward"), enabled: !!ui.detail },
    "-",
    { text: row.seen ? "Mark as Unread" : "Mark as Read", action: toggleSeen },
    { text: row.flagged ? "Unflag" : "Flag", action: toggleFlag },
    "-",
    { text: "Archive", action: archiveSelected },
    { text: "Delete", action: trashSelected },
  ]);
}

/**
 * WebKit's own menu (Reload, Inspect…) makes no sense in an app. Keep it only where it
 * helps: text fields (paste, spelling) and selected text (copy, look up).
 */
export function allowNativeMenu(e: MouseEvent, doc: Document = document): boolean {
  const t = e.target as HTMLElement | null;
  if (t && (t.isContentEditable || ["INPUT", "TEXTAREA"].includes(t.tagName))) return true;
  return !!doc.getSelection()?.toString().trim();
}
