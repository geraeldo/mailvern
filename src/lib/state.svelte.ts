import {
  api, type Account, type Detail, type FileInfo, type Folder, type MailView, type OutboxItem, type Outgoing, type SearchScope, type Summary, type View,
} from "./api";
import { formatAddr } from "./format";
import { threadText } from "./copy";
import { textToHtml, unwrapEmail } from "./html";
import { toUnix, whenLabel } from "./when";

export type ComposeMode = "new" | "reply" | "replyAll" | "forward" | "draft";
/** A message to reopen in the composer: a saved draft, or one taken back out of the outbox. */
export type Restore = { message: Outgoing; attachments: FileInfo[]; unsaved?: boolean };
export type ComposeInit = { mode: ComposeMode; source?: Detail; accountId?: number; to?: string[]; restore?: Restore; key: number };
export type SettingsSection = "general" | "rules" | "ai" | "agents" | "shortcuts";
export type SyncStatus = { state: "syncing" | "idle" | "error"; message: string | null };
/** One thing AI search did, shown while it works. */
export type AiStep = { kind: "search" | "read" | "attachment" | "count" | "thread" | "folders" | "note"; text: string; done: boolean; error?: boolean };
/** A labelled fact in an AI answer ("Amount": "Rp 1.500.000 [1]"). */
export type AiFact = { label: string; value: string };
/** One question and its answer. Citations [n] in the text refer to `sources[n - 1]`. */
export type AiTurn = {
  question: string;
  running: boolean;
  steps: AiStep[];
  /** The one-line direct answer. */
  answer: string;
  /** More explanation, with citations; may hold "- " list lines. */
  details: string;
  facts: AiFact[];
  /** Message ids. While running: what it has read so far; when done: the cited sources. */
  sources: number[];
  error: string | null;
  cost: number;
};
/** An AI search: a question and its follow-ups. */
export type AiSearch = { turns: AiTurn[]; model: string };

/** The turn whose sources the list shows: the newest one that has any. */
export function aiSourceTurn(a: AiSearch | null): AiTurn | null {
  if (!a) return null;
  for (let i = a.turns.length - 1; i >= 0; i--) if (a.turns[i].sources.length) return a.turns[i];
  return null;
}
export const aiRunning = (a: AiSearch | null) => !!a?.turns.some((t) => t.running);
export type Picker = { kind: "snooze"; rows: Summary[] } | { kind: "move"; rows: Summary[]; accountId: number };

const PAGE = 200;

export const ui = $state({
  accounts: [] as Account[],
  folders: [] as Folder[],
  view: { kind: "unified", role: "inbox" } as View,
  list: [] as Summary[],
  hasMore: false,
  listReady: false,
  loadingOlder: false,
  /** The open row (a message, or a conversation by its newest message). */
  selectedId: null as number | null,
  /** Rows picked with ⌘- or ⇧-click. Zero or one means just `selectedId`. */
  selectedIds: [] as number[],
  /** The open message: what Reply, Forward and the menus act on. */
  detail: null as Detail | null,
  /** The open conversation, oldest first. */
  thread: [] as Summary[],
  /** Loaded bodies of the open conversation's messages. */
  details: {} as Record<number, Detail>,
  /** Messages shown expanded when the conversation opened: the opened one and the unread ones. */
  expandIds: [] as number[],
  detailLoading: false,
  detailError: null as string | null,
  query: "",
  /** Where the search looks: starts at the open folder. */
  searchScope: { kind: "all" } as SearchScope,
  searchingServer: false,
  /** AI search results, shown in the list instead of the word search while set. */
  ai: null as AiSearch | null,
  compose: null as ComposeInit | null,
  /** App settings (shortcuts and, later, other preferences). */
  settings: null as SettingsSection | null,
  /** Account settings: the account being edited, or "new". */
  accountSettings: null as number | "new" | null,
  picker: null as Picker | null,
  status: {} as Record<number, SyncStatus>,
  toast: null as { text: string; undo?: () => void } | null,
  trustedSenders: [] as string[],
  outbox: [] as OutboxItem[],
  outboxSelected: null as number | null,
  /** Group messages into conversations. */
  threaded: true,
  /** Seconds a sent message can be taken back. */
  undoSend: 10,
  /** Show sender logos and website icons, looked up from the sender's own domain. */
  senderIcons: true,
});

export function openSettings(section: SettingsSection = "general") {
  ui.accountSettings = null;
  ui.settings = section;
}

/** Opens account settings on the given account, "new" for the add form, or the first account. */
export function openAccountSettings(target: number | "new" | null = null) {
  ui.settings = null;
  ui.accountSettings = target ?? ui.accounts[0]?.id ?? "new";
}

export function accountLabel(a: Account): string {
  return a.label?.trim() || a.name?.trim() || a.email;
}

export function accountById(id: number): Account | undefined {
  return ui.accounts.find((a) => a.id === id);
}

export function folderById(id: number): Folder | undefined {
  return ui.folders.find((f) => f.id === id);
}

/** Standard folders shown under All Accounts, in sidebar order. */
export const unifiedRoles = [
  { role: "inbox", name: "Inbox" },
  { role: "snoozed", name: "Snoozed" },
  { role: "drafts", name: "Drafts" },
  { role: "sent", name: "Sent" },
  { role: "archive", name: "Archive" },
  { role: "junk", name: "Junk" },
  { role: "trash", name: "Trash" },
];

/** The search scope matching a view: that folder, or that standard folder across accounts. */
export function scopeForView(v: View): SearchScope {
  if (v.kind === "folder") return { kind: "folder", folderId: v.folderId };
  if (v.kind === "unified") return { kind: "unified", role: v.role };
  return { kind: "all" };
}

export const scopeKey = (s: SearchScope) =>
  s.kind === "folder" ? `f${s.folderId}` : s.kind === "account" ? `a${s.accountId}` : s.kind === "unified" ? `u${s.role}` : "all";

const unifiedPlural: Record<string, string> = { inbox: "All Inboxes", drafts: "All Drafts" };

export function scopeLabel(s: SearchScope): string {
  if (s.kind === "all") return "All Mailboxes";
  if (s.kind === "account") {
    const a = accountById(s.accountId);
    return a ? accountLabel(a) : "Account";
  }
  if (s.kind === "unified") return unifiedPlural[s.role] ?? `All ${unifiedRoles.find((r) => r.role === s.role)?.name ?? s.role}`;
  return folderById(s.folderId)?.name ?? "Folder";
}

/** Scopes offered while searching: everything, then narrower ones around the open view. */
export function searchScopes(): SearchScope[] {
  const out: SearchScope[] = [{ kind: "all" }];
  const v = ui.view;
  if (v.kind === "folder") {
    const f = folderById(v.folderId);
    if (f && ui.accounts.length > 1) out.push({ kind: "account", accountId: f.accountId });
    out.push({ kind: "folder", folderId: v.folderId });
  } else {
    if (v.kind === "unified") out.push({ kind: "unified", role: v.role });
    if (ui.accounts.length > 1) for (const a of ui.accounts) out.push({ kind: "account", accountId: a.id });
  }
  if (!out.some((s) => scopeKey(s) === scopeKey(ui.searchScope))) out.push(ui.searchScope);
  return out;
}

export function setSearchScope(s: SearchScope) {
  if (scopeKey(s) === scopeKey(ui.searchScope)) return;
  ui.searchScope = s;
  if (ui.query.trim()) setQuery(ui.query);
}

/** Search starts in the open folder. */
export function beginSearch() {
  if (!ui.query.trim()) ui.searchScope = scopeForView(ui.view);
}

export function viewTitle(): string {
  if (ui.query.trim()) return "Search";
  if (ui.view.kind === "outbox") return "Scheduled";
  if (ui.view.kind === "unified") {
    const role = ui.view.role;
    return unifiedRoles.find((r) => r.role === role)?.name ?? "";
  }
  if (ui.view.kind === "folder") {
    const id = ui.view.folderId;
    return folderById(id)?.name ?? "";
  }
  return "";
}

/** The role of the folder a row is in ("snoozed", "drafts", "junk"…). */
export function roleOf(row: { folderId: number } | null | undefined): string | null {
  return row ? (folderById(row.folderId)?.role ?? null) : null;
}

export async function loadAccounts() {
  ui.accounts = await api.accounts();
}

export async function loadPrefs() {
  const [undo, threads, icons] = await Promise.all([
    api.getSetting("undo_send"),
    api.getSetting("threads"),
    api.getSetting("sender_icons"),
  ]);
  const n = Number(undo);
  if (undo != null && Number.isFinite(n)) ui.undoSend = n;
  ui.threaded = threads !== "off";
  ui.senderIcons = icons !== "off";
}

export async function setSenderIcons(on: boolean) {
  ui.senderIcons = on;
  await api.setSetting("sender_icons", on ? "on" : "off");
}

export async function setUndoSend(seconds: number) {
  ui.undoSend = seconds;
  await api.setSetting("undo_send", String(seconds));
}

export async function setThreaded(on: boolean) {
  ui.threaded = on;
  await api.setSetting("threads", on ? "on" : "off");
  if (ui.view.kind !== "outbox") setView(ui.view);
}

export async function reorderAccounts(ids: number[]) {
  const byId = new Map(ui.accounts.map((a) => [a.id, a]));
  ui.accounts = ids.map((id) => byId.get(id)).filter((a): a is Account => !!a);
  try {
    await api.reorderAccounts(ids);
  } catch (e) {
    toast(String(e));
  }
  await loadAccounts();
}

/** `visibleIds` is the new order of the folders on screen; hidden ones keep their slots. */
export async function reorderFolders(accountId: number, visibleIds: number[]) {
  const all = ui.folders.filter((f) => f.accountId === accountId);
  const shown = new Set(visibleIds);
  const queue = [...visibleIds];
  const full = all.map((f) => (shown.has(f.id) ? queue.shift()! : f.id));
  const byId = new Map(all.map((f) => [f.id, f]));
  const reordered = full.map((id, i) => ({ ...byId.get(id)!, position: i }));
  ui.folders = [...ui.folders.filter((f) => f.accountId !== accountId), ...reordered];
  try {
    await api.reorderFolders(accountId, full);
  } catch (e) {
    toast(String(e));
  }
  await loadFolders();
}

export async function loadFolders() {
  ui.folders = await api.folders();
}

export async function loadOutbox() {
  try {
    ui.outbox = await api.outbox();
  } catch {}
  if (ui.outboxSelected != null && !ui.outbox.some((o) => o.id === ui.outboxSelected)) ui.outboxSelected = null;
  // Nothing left to show: leave the empty Scheduled view.
  if (ui.view.kind === "outbox" && !ui.outbox.length) setView({ kind: "unified", role: "inbox" });
}

/** Server search results for the current query and scope, kept so list refreshes don't ask again. */
let remote: { q: string; scope: string; rows: Summary[] } | null = null;

function mergeRows(a: Summary[], b: Summary[]): Summary[] {
  const seen = new Set(a.map((r) => r.id));
  return [...a, ...b.filter((r) => !seen.has(r.id))].sort((x, y) => y.date - x.date || y.id - x.id);
}

let listSeq = 0;
export async function loadList(reset = true) {
  const my = ++listSeq;
  if (ui.view.kind === "outbox") {
    await loadOutbox();
    if (my !== listSeq) return;
    ui.list = [];
    ui.hasMore = false;
    ui.listReady = true;
    return;
  }
  const view = ui.view;
  const q = ui.query.trim();
  let rows: Summary[];
  try {
    if (ui.ai) {
      if (!reset) return;
      // The answer's sources, in citation order, each with the size of its conversation.
      const ids = aiSourceTurn(ui.ai)?.sources ?? [];
      rows = ids.length ? await api.summaries(ids) : [];
      rows = await Promise.all(
        rows.map(async (r) => {
          const thread = await api.thread(r.id).catch(() => []);
          return { ...r, count: Math.max(1, thread.length) };
        }),
      );
      if (my !== listSeq) return;
      ui.list = rows;
      ui.hasMore = false;
      ui.listReady = true;
      return;
    } else if (q) {
      if (!reset) return;
      const scope = ui.searchScope;
      rows = await api.search(q, scope);
      if (remote?.q === q && remote.scope === scopeKey(scope)) rows = mergeRows(rows, remote.rows);
    } else {
      const limit = reset ? Math.max(PAGE, ui.list.length) : PAGE;
      rows = await api.messages(view, null, reset ? 0 : ui.list.length, limit, ui.threaded);
      if (my === listSeq) ui.hasMore = rows.length === limit;
    }
  } catch (e) {
    if (my === listSeq) {
      ui.listReady = true;
      toast(String(e));
    }
    return;
  }
  if (my !== listSeq) return;
  ui.list = reset ? rows : [...ui.list, ...rows];
  ui.listReady = true;
  if (q) ui.hasMore = false;
  // A conversation's row moves to its newest message when one arrives: follow it.
  if (ui.selectedId != null && !ui.list.some((m) => m.id === ui.selectedId)) {
    const holder = ui.list.find((m) => m.ids.includes(ui.selectedId!));
    if (holder) ui.selectedId = holder.id;
  }
  ui.selectedIds = ui.selectedIds.filter((id) => ui.list.some((m) => m.id === id));
}

/** Views whose folders have no older mail left on the server. */
const reachedStart = new Set<string>();
const viewKey = (v: View) => (v.kind === "folder" ? `f${v.folderId}` : v.kind === "unified" ? `u${v.role}` : v.kind);

/** Next page from the local cache; past its end, older mail from the server. */
export async function loadMore() {
  if (ui.query.trim() || ui.loadingOlder || ui.view.kind === "outbox") return;
  if (ui.hasMore) {
    await loadList(false);
    return;
  }
  const view = ui.view;
  const key = viewKey(view);
  if (reachedStart.has(key)) return;
  ui.loadingOlder = true;
  try {
    const n = await api.loadOlder(view);
    if (n === 0) reachedStart.add(key);
    else if (viewKey(ui.view) === key) {
      ui.hasMore = true;
      await loadList(false);
    }
  } catch (e) {
    toast(String(e));
    reachedStart.add(key); // don't retry on every scroll; a view change resets this
  } finally {
    ui.loadingOlder = false;
  }
}

function clearOpen() {
  ui.selectedId = null;
  ui.selectedIds = [];
  ui.detail = null;
  ui.thread = [];
  ui.details = {};
  ui.outboxSelected = null;
}

export function setView(v: View) {
  reachedStart.delete(viewKey(v));
  ui.view = v;
  // Picking a folder while searching searches there instead.
  if (ui.query.trim() && v.kind !== "outbox") {
    ui.searchScope = scopeForView(v);
    setQuery(ui.query);
    return;
  }
  ui.query = "";
  ui.ai = null;
  ui.list = [];
  ui.listReady = false;
  remote = null;
  ui.searchingServer = false;
  clearOpen();
  loadList(true);
}

/** Shows an AI search's results (or its progress) in the list. */
export function showAi(a: AiSearch | null) {
  ui.ai = a;
  ui.list = [];
  remote = null;
  ui.searchingServer = false;
  clearTimeout(serverTimer);
  clearOpen();
  loadList(true);
}

/** Reloads the AI sources in the list, leaving any open message open. */
export function refreshAi() {
  if (ui.ai) loadList(true);
}

let serverTimer: ReturnType<typeof setTimeout> | undefined;
export function setQuery(q: string) {
  ui.ai = null;
  ui.query = q;
  // The last results stay until the new ones replace them, so the list doesn't blank between keys.
  remote = null;
  ui.searchingServer = false;
  clearOpen();
  clearTimeout(serverTimer);
  loadList(true);
  const t = q.trim();
  // The server is slower and costlier: ask once typing settles.
  if (t) serverTimer = setTimeout(() => searchServer(t), 600);
}

async function searchServer(q: string) {
  if (!ui.accounts.length) return;
  const scope = ui.searchScope;
  const key = scopeKey(scope);
  const current = () => !ui.ai && ui.query.trim() === q && scopeKey(ui.searchScope) === key;
  ui.searchingServer = true;
  try {
    const rows = await api.searchServer(q, scope);
    if (!current()) return;
    remote = { q, scope: key, rows };
    ui.list = mergeRows(ui.list, rows);
  } catch (e) {
    if (current()) toast(`Server search failed: ${e}`);
  } finally {
    if (current()) ui.searchingServer = false;
  }
}

let detailSeq = 0;
/** Opens a row: its message, and the conversation around it. */
export async function openMessage(id: number | null) {
  ui.selectedId = id;
  ui.selectedIds = [];
  cursor = null;
  const my = ++detailSeq;
  ui.detailError = null;
  if (id == null) {
    ui.detail = null;
    ui.thread = [];
    ui.details = {};
    ui.detailLoading = false;
    return;
  }
  // Only show a spinner if the body isn't already local.
  const spinner = setTimeout(() => {
    if (my === detailSeq) {
      ui.detail = null;
      ui.detailLoading = true;
    }
  }, 120);
  try {
    const [d, thread] = await Promise.all([api.message(id), api.thread(id).catch(() => [] as Summary[])]);
    if (my !== detailSeq) return;
    ui.details = { [id]: d };
    ui.detail = d;
    const t = thread.length ? thread : [d];
    ui.expandIds = [id, ...t.filter((m) => !m.seen).map((m) => m.id)];
    ui.thread = t;
  } catch (e) {
    if (my === detailSeq) {
      ui.detail = null;
      ui.thread = [];
      ui.detailError = String(e);
    }
  } finally {
    clearTimeout(spinner);
    if (my === detailSeq) ui.detailLoading = false;
  }
  const row = ui.list.find((m) => m.id === id);
  if (row && !row.seen && my === detailSeq) {
    row.seen = true;
    if (ui.detail) ui.detail.seen = true;
    for (const t of ui.thread) if (row.ids.includes(t.id)) t.seen = true;
    api.setSeen(row.ids, true).then(loadFolders).catch((e) => {
      toast(String(e));
      loadList(true);
      loadFolders();
    });
  }
}

/** Picks up new or changed messages in the open conversation. */
export async function refreshThread() {
  const id = ui.selectedId;
  if (id == null || ui.selectedIds.length > 1) return;
  const anchor = ui.detail?.id ?? id;
  try {
    const t = await api.thread(anchor);
    if (ui.selectedId === id && t.length) ui.thread = t;
  } catch {}
}

export function move(delta: number) {
  if (!ui.list.length) return;
  const i = ui.list.findIndex((m) => m.id === ui.selectedId);
  const next = i < 0 ? 0 : Math.min(ui.list.length - 1, Math.max(0, i + delta));
  openMessage(ui.list[next].id);
}

// ---------- selection ----------

/** The moving end of a ⇧-selection; `selectedId` is where it started. */
let cursor: number | null = null;

function selectRange(fromId: number, toId: number) {
  const a = ui.list.findIndex((m) => m.id === fromId);
  const b = ui.list.findIndex((m) => m.id === toId);
  if (a < 0 || b < 0) return;
  ui.selectedIds = ui.list.slice(Math.min(a, b), Math.max(a, b) + 1).map((m) => m.id);
}

/** Click on a row: open it; ⌘ adds or removes it from the selection; ⇧ selects a range. */
export function clickRow(m: Summary, e: MouseEvent) {
  if (e.metaKey) {
    const base = ui.selectedIds.length ? ui.selectedIds : ui.selectedId != null ? [ui.selectedId] : [];
    const next = base.includes(m.id) ? base.filter((id) => id !== m.id) : [...base, m.id];
    if (next.length <= 1) openMessage(next[0] ?? null);
    else ui.selectedIds = next;
  } else if (e.shiftKey && ui.selectedId != null) {
    cursor = m.id;
    selectRange(ui.selectedId, m.id);
  } else {
    openMessage(m.id);
  }
}

/** ⇧↓ / ⇧↑: grow or shrink the selection. */
export function extendSelection(delta: number) {
  if (!ui.list.length) return;
  if (ui.selectedId == null) {
    openMessage(ui.list[0].id);
    return;
  }
  const from = cursor ?? ui.selectedId;
  const i = ui.list.findIndex((m) => m.id === from);
  const next = ui.list[Math.min(ui.list.length - 1, Math.max(0, i + delta))];
  cursor = next.id;
  selectRange(ui.selectedId, next.id);
}

export function selectAll() {
  if (ui.list.length) ui.selectedIds = ui.list.map((m) => m.id);
}

/** Leaves a multiple selection, back to the open row. */
export function clearSelection() {
  if (ui.selectedIds.length <= 1) return false;
  ui.selectedIds = [];
  cursor = null;
  return true;
}

export const isSelected = (id: number) => (ui.selectedIds.length > 1 ? ui.selectedIds.includes(id) : ui.selectedId === id);

/** The rows an action applies to: the selection, or the open row. */
export function selectedRows(): Summary[] {
  if (ui.selectedIds.length > 1) return ui.list.filter((m) => ui.selectedIds.includes(m.id));
  const row = ui.list.find((m) => m.id === ui.selectedId);
  if (row) return [row];
  return ui.detail ? [{ ...ui.detail, ids: [ui.detail.id], count: 1 }] : [];
}

/** Every message behind some rows (a conversation row stands for all its messages in the view). */
export const idsOf = (rows: Summary[]) => [...new Set(rows.flatMap((r) => (r.ids?.length ? r.ids : [r.id])))];

/** Takes rows off the list and opens the next one. Returns their message ids. */
function removeRows(rows: Summary[]): number[] {
  const gone = new Set(rows.map((r) => r.id));
  const first = ui.list.findIndex((m) => gone.has(m.id));
  ui.list = ui.list.filter((m) => !gone.has(m.id));
  ui.selectedIds = [];
  const next = first < 0 ? null : (ui.list[first] ?? ui.list[first - 1] ?? null);
  openMessage(next ? next.id : null);
  return idsOf(rows);
}

// ---------- actions ----------

let toastTimer: ReturnType<typeof setTimeout> | undefined;
export function toast(text: string, opts: { undo?: () => void; ms?: number } = {}) {
  ui.toast = { text, undo: opts.undo };
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (ui.toast = null), opts.ms ?? 3200);
}

/** Copies the open conversation as plain text (for an AI chat, notes, a ticket). */
export async function copyThread() {
  const d = ui.detail;
  if (!d || ui.selectedIds.length > 1) return;
  const thread = ui.thread.length ? ui.thread : [d];
  try {
    await api.copyText(await threadText(d.subject, thread, ui.details));
    toast(thread.length > 1 ? `Copied ${thread.length} messages` : "Copied message");
  } catch (e) {
    toast(`Couldn't copy: ${e}`);
  }
}

export function dismissToast() {
  clearTimeout(toastTimer);
  ui.toast = null;
}

/** Runs the toast's Undo and closes it. */
export function undoToast() {
  const undo = ui.toast?.undo;
  dismissToast();
  undo?.();
}

async function run(p: Promise<unknown>): Promise<boolean> {
  let ok = true;
  try {
    await p;
  } catch (e) {
    ok = false;
    toast(String(e));
    loadList(true);
  }
  loadFolders();
  return ok;
}

const counted = (verb: string, n: number) => (n > 1 ? `${verb} ${n} items` : verb);

/** Removes the selected rows and runs `call` on their messages. */
function bulk(verb: string, call: (ids: number[]) => Promise<unknown>) {
  const rows = selectedRows();
  if (!rows.length) return;
  const ids = removeRows(rows);
  run(call(ids)).then((ok) => {
    if (ok) toast(counted(verb, rows.length));
  });
}

/** Checks the server for new mail in just this folder (or unified folder). */
export function checkForMail(view: MailView) {
  api.syncView(view).catch((e) => toast(String(e)));
}

/** Checks the server for new mail in every folder of one account. */
export function checkAccountForMail(accountId: number) {
  api.syncAccount(accountId).catch((e) => toast(String(e)));
}

export const archiveSelected = () => bulk("Archived", api.archive);
export const trashSelected = () => bulk("Deleted", api.trash);

/** True when the selection sits in a Junk folder, so the junk action means "Not Junk". */
export function selectedIsJunk(): boolean {
  return roleOf(selectedRows()[0]) === "junk";
}

/** Moves the selection to Junk, or back to the Inbox if it's already in Junk. */
export function toggleJunk() {
  if (selectedIsJunk()) bulk("Moved to Inbox", api.notJunk);
  else bulk("Moved to Junk", api.junk);
}

/** Read / unread. Marking read covers whole conversations; unread marks just their newest message. */
export function toggleSeen() {
  const rows = selectedRows();
  if (!rows.length) return;
  const seen = rows.some((r) => !r.seen);
  for (const r of rows) r.seen = seen;
  const ids = seen ? idsOf(rows) : rows.map((r) => r.id);
  for (const t of ui.thread) if (ids.includes(t.id)) t.seen = seen;
  if (ui.detail && ids.includes(ui.detail.id)) ui.detail.seen = seen;
  run(api.setSeen(ids, seen));
}

/** Flags the newest message of each conversation; unflagging clears every message. */
export function toggleFlag() {
  const rows = selectedRows();
  if (!rows.length) return;
  const flagged = rows.some((r) => !r.flagged);
  for (const r of rows) r.flagged = flagged;
  const ids = flagged ? rows.map((r) => r.id) : idsOf(rows);
  for (const t of ui.thread) if (ids.includes(t.id)) t.flagged = flagged;
  if (ui.detail && ids.includes(ui.detail.id)) ui.detail.flagged = flagged;
  run(api.setFlagged(ids, flagged));
}

/** Marks everything in a folder (or a unified folder) as read, here and on the server. */
export async function markAllRead(view: MailView) {
  const inView = (fid: number) =>
    view.kind === "folder" ? fid === view.folderId : view.kind === "unified" && folderById(fid)?.role === view.role;
  for (const m of ui.list) if (inView(m.folderId)) m.seen = true;
  if (ui.detail && inView(ui.detail.folderId)) ui.detail.seen = true;
  for (const f of ui.folders) if (inView(f.id)) f.unread = 0;
  await run(api.markRead(view));
}

export function snoozeSelected() {
  const rows = selectedRows();
  if (rows.length) ui.picker = { kind: "snooze", rows };
}

export function snoozeRows(rows: Summary[], until: Date) {
  const gone = new Set(rows.map((r) => r.id));
  const shown = ui.list.filter((m) => gone.has(m.id));
  const ids = shown.length ? removeRows(shown) : idsOf(rows);
  run(api.snooze(ids, toUnix(until))).then((ok) => {
    if (ok) toast(`Snoozed until ${whenLabel(until)}`);
  });
}

export function unsnoozeSelected() {
  bulk("Unsnoozed", api.unsnooze);
}

export function moveSelected() {
  const rows = selectedRows();
  if (!rows.length) return;
  const accounts = new Set(rows.map((r) => r.accountId));
  if (accounts.size > 1) {
    toast("Select messages from one account to move them");
    return;
  }
  ui.picker = { kind: "move", rows, accountId: rows[0].accountId };
}

export function moveRows(rows: Summary[], folder: Folder) {
  const gone = new Set(rows.map((r) => r.id));
  const shown = ui.list.filter((m) => gone.has(m.id));
  const ids = shown.length ? removeRows(shown) : idsOf(rows);
  run(api.moveTo(ids, folder.id)).then((ok) => {
    if (ok) toast(`Moved to ${folder.name}`);
  });
}

// ---------- writing ----------

let composeKey = 0;
export function compose(mode: ComposeMode = "new", extra: Partial<ComposeInit> = {}) {
  if (!ui.accounts.length) {
    openAccountSettings("new");
    return;
  }
  const source = mode === "new" || mode === "draft" ? undefined : (ui.detail ?? undefined);
  if ((mode === "reply" || mode === "replyAll" || mode === "forward") && !(extra.source ?? source)) return;
  ui.compose = { mode, source, key: ++composeKey, ...extra };
}

/** Opens a saved draft in the composer. */
export async function editDraft(d: Detail) {
  let attachments: FileInfo[] = [];
  if (d.attachments.length) {
    try {
      attachments = await api.attachmentFiles(d.id);
    } catch (e) {
      toast(String(e));
    }
  }
  compose("draft", {
    restore: {
      message: {
        accountId: d.accountId,
        to: d.to.map(formatAddr),
        cc: d.cc.map(formatAddr),
        bcc: d.bcc.map(formatAddr),
        subject: d.subject,
        html: d.html ? unwrapEmail(d.html) : textToHtml(d.text ?? ""),
        text: "",
        attachments: [],
        inReplyTo: d.inReplyTo,
        references: d.references,
        messageId: d.messageId,
      },
      attachments,
    },
  });
}

/** Takes a message out of the outbox and reopens it in the composer (undo send, or edit). */
export async function editQueued(id: number) {
  try {
    const c = await api.cancelSend(id);
    compose("draft", { restore: { message: { ...c.message, html: unwrapEmail(c.message.html) }, attachments: c.attachments, unsaved: true } });
  } catch (e) {
    toast(String(e));
  }
  loadOutbox();
}

export async function deleteQueued(id: number) {
  try {
    await api.cancelSend(id);
    toast("Deleted");
  } catch (e) {
    toast(String(e));
  }
  loadOutbox();
}

export async function sendQueuedNow(id: number) {
  try {
    await api.sendNow(id);
  } catch (e) {
    toast(String(e));
  }
  loadOutbox();
}

export async function loadTrusted() {
  try {
    ui.trustedSenders = JSON.parse((await api.getSetting("trusted_senders")) || "[]");
  } catch {
    ui.trustedSenders = [];
  }
}

export async function trustSender(email: string) {
  const e = email.toLowerCase();
  if (!ui.trustedSenders.includes(e)) {
    ui.trustedSenders = [...ui.trustedSenders, e];
    await api.setSetting("trusted_senders", JSON.stringify(ui.trustedSenders));
  }
}
