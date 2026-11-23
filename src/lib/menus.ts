import { Menu } from "@tauri-apps/api/menu";
import type { Account, MailView, Summary } from "./api";
import {
  ui, openAccountSettings, openMessage, compose, archiveSelected, trashSelected, toggleJunk, selectedIsJunk, toggleSeen, toggleFlag, markAllRead,
  checkForMail, checkAccountForMail, copyThread, selectedRows, snoozeSelected, unsnoozeSelected, moveRows, moveSelected, roleOf, editDraft,
} from "./state.svelte";

type Item = { text: string; action: () => void; enabled?: boolean } | { text: string; items: Item[] } | "-";

function toOptions(items: Item[]): any[] {
  return items.map((i) =>
    i === "-"
      ? { item: "Separator" as const }
      : "items" in i
        ? { text: i.text, items: toOptions(i.items) }
        : { text: i.text, enabled: i.enabled ?? true, action: i.action },
  );
}

async function popup(items: Item[]) {
  const menu = await Menu.new({ items: toOptions(items) });
  await menu.popup();
}

export function accountMenu(acc: Account, collapsed: boolean, toggle: () => void) {
  return popup([
    { text: "Account Settings…", action: () => openAccountSettings(acc.id) },
    { text: "Add Account…", action: () => openAccountSettings("new") },
    { text: "Check for Mail", action: () => checkAccountForMail(acc.id) },
    "-",
    { text: collapsed ? "Expand" : "Collapse", action: toggle },
  ]);
}

export function folderMenu(view: MailView, unread: number) {
  return popup([
    { text: "Check for Mail", action: () => checkForMail(view) },
    { text: "Mark All as Read", action: () => markAllRead(view), enabled: unread > 0 },
  ]);
}

export async function messageMenu(m: Summary) {
  // Right-clicking inside a multiple selection acts on all of it; elsewhere, on that row.
  const multi = ui.selectedIds.length > 1 && ui.selectedIds.includes(m.id);
  if (!multi && (ui.selectedId !== m.id || !ui.detail)) await openMessage(m.id);
  const rows = selectedRows();
  const one = rows.length === 1;
  const seenAll = rows.every((r) => r.seen);
  const flaggedAll = rows.every((r) => r.flagged);
  const snoozed = rows.every((r) => roleOf(r) === "snoozed");
  const isDraft = one && roleOf(rows[0]) === "drafts";
  const accounts = new Set(rows.map((r) => r.accountId));
  const current = new Set(rows.map((r) => r.folderId));
  const targets = accounts.size === 1
    ? ui.folders.filter((f) => f.accountId === rows[0].accountId && !current.has(f.id) && f.role !== "snoozed")
    : [];
  return popup([
    ...(isDraft && ui.detail ? [{ text: "Edit Draft", action: () => ui.detail && editDraft(ui.detail) }, "-" as const] : []),
    { text: "Reply", action: () => compose("reply"), enabled: one && !!ui.detail },
    { text: "Reply All", action: () => compose("replyAll"), enabled: one && !!ui.detail },
    { text: "Forward", action: () => compose("forward"), enabled: one && !!ui.detail },
    "-",
    { text: ui.thread.length > 1 ? "Copy Conversation" : "Copy Message", action: copyThread, enabled: one && !!ui.detail },
    "-",
    { text: seenAll ? "Mark as Unread" : "Mark as Read", action: toggleSeen },
    { text: flaggedAll ? "Unflag" : "Flag", action: toggleFlag },
    "-",
    { text: "Archive", action: archiveSelected },
    { text: "Delete", action: trashSelected },
    { text: selectedIsJunk() ? "Not Junk" : "Move to Junk", action: toggleJunk },
    targets.length
      ? { text: "Move to", items: [...targets.map((f) => ({ text: f.name, action: () => moveRows(rows, f) })), "-" as const, { text: "Other…", action: moveSelected }] }
      : { text: "Move to…", action: moveSelected, enabled: accounts.size === 1 },
    "-",
    snoozed ? { text: "Unsnooze", action: unsnoozeSelected } : { text: "Snooze…", action: snoozeSelected },
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
