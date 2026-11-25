import { Menu } from "@tauri-apps/api/menu";
import type { Account, Addr, Detail, MailView, Summary } from "./api";
import { api } from "./api";
import { formatAddr } from "./format";
import { openRuleEditor, ruleFromAddress, ruleFromMessage, describeFromMessage, type FromMessage } from "./rules.svelte";
import {
  ui, openAccountSettings, openMessage, compose, archiveSelected, trashSelected, toggleJunk, selectedIsJunk, toggleSeen, toggleFlag, markAllRead,
  toast, setQuery, isSenderTrusted, trustSender, trustDomain, checkForMail, checkAccountForMail, copyThread, selectedRows, snoozeSelected, unsnoozeSelected, moveRows, moveSelected, roleOf, editDraft,
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

/** Collapse/expand one sidebar group, plus every group at once. */
export type Fold = { collapsed: boolean; toggle: () => void; allCollapsed: boolean; setAll: (collapsed: boolean) => void };

function foldItems(f: Fold): Item[] {
  return [
    { text: f.collapsed ? "Expand" : "Collapse", action: f.toggle },
    f.allCollapsed ? { text: "Expand All", action: () => f.setAll(false) } : { text: "Collapse All", action: () => f.setAll(true) },
  ];
}

export function accountMenu(acc: Account, fold: Fold) {
  return popup([
    { text: "Account Settings…", action: () => openAccountSettings(acc.id) },
    { text: "Add Account…", action: () => openAccountSettings("new") },
    { text: "Check for Mail", action: () => checkAccountForMail(acc.id) },
    "-",
    ...foldItems(fold),
  ]);
}

/** Right-click on the All Accounts header. */
export function allAccountsMenu(fold: Fold) {
  return popup([
    { text: "Add Account…", action: () => openAccountSettings("new") },
    "-",
    ...foldItems(fold),
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
    ...(one && ui.detail ? ["-" as const, { text: "Create Rule", items: ruleItems() }] : []),
  ]);
}

/** "Create Rule" choices for the open message: by its sender, domain, list or subject. */
function ruleItems(): Item[] {
  const d = ui.detail!;
  const domain = d.fromAddr.split("@")[1] ?? "";
  const make = (kind: FromMessage) => () => ui.detail && openRuleEditor(ruleFromMessage(ui.detail, kind));
  return [
    { text: `From ${d.fromAddr}…`, action: make("sender"), enabled: !!d.fromAddr },
    { text: `From Anyone at ${domain}…`, action: make("domain"), enabled: !!domain },
    ...(d.listId ? [{ text: "From This Mailing List…", action: make("list") }] : []),
    { text: "With This Subject…", action: make("subject"), enabled: !!d.subject.trim() },
    "-",
    { text: "Describe a Rule…", action: () => ui.detail && describeFromMessage(ui.detail) },
    { text: "New Rule…", action: () => openRuleEditor() },
  ];
}

async function copy(text: string, what: string) {
  try {
    await api.copyText(text);
    toast(`Copied ${what}`);
  } catch (e) {
    toast(`Couldn't copy: ${e}`);
  }
}

/**
 * Right-click on a sender or recipient in the reader. `d` is the message it appears in
 * (for replying), `sender` whether this is its From address.
 */
export function addressMenu(e: MouseEvent, a: Addr, d?: Detail, sender = false) {
  e.preventDefault();
  e.stopPropagation();
  const name = a.name?.trim();
  const email = a.email.toLowerCase();
  const mine = ui.accounts.some((x) => x.email.toLowerCase() === email);
  const domain = email.split("@")[1] ?? "";
  const accountId = d?.accountId ?? null;
  const trusted = isSenderTrusted(email);
  return popup([
    { text: "Send Email", action: () => compose("new", { to: [formatAddr(a)] }) },
    ...(d && !mine ? [{ text: `Reply to ${name || a.email} Only`, action: () => compose("reply", { source: d, to: [formatAddr(a)] }) }] : []),
    "-",
    { text: "Copy Address", action: () => copy(a.email, "address") },
    ...(name ? [
      { text: "Copy Name", action: () => copy(name, "name") },
      { text: "Copy Name and Address", action: () => copy(formatAddr(a), "name and address") },
    ] : []),
    "-",
    { text: "Search for This Address", action: () => setQuery(a.email) },
    ...(mine ? [] : [
      { text: "Create Rule", items: [
        { text: `From ${a.email}…`, action: () => openRuleEditor(ruleFromAddress(a.email, name ?? null, accountId, "sender")) },
        { text: `From Anyone at ${domain}…`, action: () => openRuleEditor(ruleFromAddress(a.email, name ?? null, accountId, "domain")), enabled: !!domain },
      ] },
    ]),
    ...(sender && !mine && !trusted ? [
      "-" as const,
      { text: "Always Load Images from This Sender", action: () => trustSender(a.email) },
      ...(domain ? [{ text: `Always Load Images from @${domain}`, action: () => trustDomain(domain) }] : []),
    ] : []),
  ]);
}

/**
 * WebKit's own menu (Reload, Inspect…) makes no sense in an app. Keep it only where it
 * helps: text fields (paste, spelling) and selected text (copy, look up).
 */
export function allowNativeMenu(e: MouseEvent): boolean {
  const t = e.target as HTMLElement | null;
  if (t && (t.isContentEditable || ["INPUT", "TEXTAREA"].includes(t.tagName))) return true;
  return !!document.getSelection()?.toString().trim();
}
