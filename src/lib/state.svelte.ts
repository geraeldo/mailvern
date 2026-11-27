import {
  api, type Account, type AccountBadgeMode, type Detail, type FileInfo, type Folder, type MailView, type OutboxItem, type Outgoing, type SearchScope, type SmartCounts, type Summary, type View,
} from "./api";
import { hydrateAi } from "./ai/config.svelte";
import { hydrateBadgeMode, refreshBadge } from "./badge.svelte";
import { formatAddr } from "./format";
import { threadText } from "./copy";
import { textToHtml, unwrapEmail } from "./html";
import { hydrateShortcuts } from "./shortcuts.svelte";
import { hydrateTheme } from "./theme.svelte";
import { extractEmailAddress, isRemoteImageAllowed, validateTrustEntry, type TrustEntryResult } from "./trust";
import { toUnix, whenLabel } from "./when";
import {
  cachedDetailsFor, cachedFrameDoc, cachedRemoteImageUrls, clearMailCache, evictMessages, getCachedDetail,
  getCachedThread, hasShownImages, markCachedFolderRead, peekCachedDetail, preloadRemoteImages, putDetail,
  putThread, updateCachedFlagged, updateCachedSeen, updatePaneThread, upsertReaderPane,
} from "./mail-cache.svelte";

export type ComposeMode = "new" | "reply" | "replyAll" | "forward" | "draft";
/** A message to reopen in the composer: a saved draft, or one taken back out of the outbox. */
export type Restore = { message: Outgoing; attachments: FileInfo[]; unsaved?: boolean };
export type ComposeInit = {
  mode: ComposeMode;
  source?: Detail;
  accountId?: number;
  to?: string[];
  cc?: string[];
  bcc?: string[];
  subject?: string;
  bodyText?: string;
  attachments?: FileInfo[];
  restore?: Restore;
  key: number;
};
export type ComposeSession = ComposeInit & { minimized: boolean; expanded: boolean };
export type SettingsSection = "general" | "notifications" | "accounts" | "rules" | "ai" | "agents" | "shortcuts";
export type RemoteImagesMode = "ask" | "strip_trackers" | "always";
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
export type FolderPrompt =
  | { kind: "create"; accountId: number; parentId: number | null; parentName?: string }
  | { kind: "rename"; folder: Folder };

const PAGE = 100;
/** Initial boot page size (matches Rust preboot snapshot; covers ~4 viewports). */
const BOOT_PAGE = 40;

const STARTUP_SETTING_KEYS = [
  "theme",
  "undo_send",
  "threads",
  "sender_icons",
  "side_width",
  "list_width",
  "sidebar_collapsed",
  "remote_images",
  "auto_trust_sent",
  "trusted_senders",
  "shortcuts",
  "badge_mode",
  "ai_provider",
  "ai_search_model",
  "ai_write_model",
  "ai_private",
  "ai_configured",
];

export const ui = $state({
  accounts: [] as Account[],
  folders: [] as Folder[],
  smartCounts: { flagged: 0, unread: 0 } as SmartCounts,
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
  /** Open non-blocking composer sessions (supports multiple simultaneous drafts). */
  composers: [] as ComposeSession[],
  activeComposerKey: null as number | null,
  /** Unified app & account settings modal. */
  settings: null as SettingsSection | null,
  /** Selected account inside Settings › Accounts (account id, or "new"). */
  accountTarget: "new" as number | "new",
  /** Quick keyboard shortcut cheat-sheet overlay ("?"). */
  shortcutHelp: false,
  /** Sidebar collapsed state & persisted pane widths. */
  sidebarCollapsed: false,
  sideWidth: 214,
  listWidth: 336,
  picker: null as Picker | null,
  folderPrompt: null as FolderPrompt | null,
  sourceView: null as { id: number; subject: string; raw: string } | null,
  status: {} as Record<number, SyncStatus>,
  toast: null as { text: string; undo?: () => void; action?: { label: string; fn: () => void } } | null,
  trustedSenders: [] as string[],
  /** Whether remote images load automatically ("always") or only for trusted senders ("ask"). */
  remoteImages: "ask" as RemoteImagesMode,
  /** Automatically trust remote images from addresses you've emailed and your own accounts. */
  autoTrustSent: true,
  /** Lowercased recipient emails from Sent folders (and freshly sent mail). */
  sentRecipients: new Set<string>(),
  outbox: [] as OutboxItem[],
  outboxSelected: null as number | null,
  /** Pending MCP agent actions waiting for user approval. */
  pendingAgentActions: 0,
  /** Group messages into conversations. */
  threaded: true,
  /** Seconds a sent message can be taken back. */
  undoSend: 10,
  /** Show sender logos and website icons, looked up from the sender's own domain. */
  senderIcons: true,
});

export function openSettings(section: SettingsSection = "general") {
  ui.shortcutHelp = false;
  if (section === "accounts" && typeof ui.accountTarget === "number" && !ui.accounts.some((a) => a.id === ui.accountTarget)) {
    ui.accountTarget = ui.accounts[0]?.id ?? "new";
  }
  ui.settings = section;
}

/** Opens unified Settings directly on the given account, "new" for the add form, or the first account. */
export function openAccountSettings(target: number | "new" | null = null) {
  ui.shortcutHelp = false;
  ui.accountTarget = target ?? ui.accounts[0]?.id ?? "new";
  ui.settings = "accounts";
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
  if (ui.view.kind === "smart") return ui.view.name === "flagged" ? "Flagged" : "Unread";
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
  const prev = ui.accounts;
  const next = await api.accounts();
  if (prev.some((a) => !next.some((b) => b.id === a.id))) clearMailCache();
  ui.accounts = next;
}

function applyPrefs(settings: Record<string, string>) {
  const undo = settings["undo_send"] ?? null;
  const n = Number(undo);
  if (undo != null && Number.isFinite(n)) ui.undoSend = n;
  ui.threaded = settings["threads"] !== "off";
  ui.senderIcons = settings["sender_icons"] !== "off";
  const sideW = settings["side_width"] ?? null;
  const sw = Number(sideW);
  if (sideW != null && Number.isFinite(sw) && sw >= 170 && sw <= 320) ui.sideWidth = sw;
  const listW = settings["list_width"] ?? null;
  const lw = Number(listW);
  if (listW != null && Number.isFinite(lw) && lw >= 260 && lw <= 480) ui.listWidth = lw;
  ui.sidebarCollapsed = settings["sidebar_collapsed"] === "on";
  const remoteImg = settings["remote_images"];
  ui.remoteImages = remoteImg === "always" || remoteImg === "strip_trackers" ? remoteImg : "ask";
  ui.autoTrustSent = settings["auto_trust_sent"] !== "off";
}

export async function loadPrefs() {
  const settings = await api.getSettings([
    "undo_send", "threads", "sender_icons", "side_width",
    "list_width", "sidebar_collapsed", "remote_images", "auto_trust_sent",
  ]);
  applyPrefs(settings);
}

let bootPromise: Promise<void> | null = null;

/** Loads all initial UI state in a single IPC round-trip (consuming Rust's parallel preboot snapshot). */
export function bootstrapApp(): Promise<void> {
  if (bootPromise) return bootPromise;
  bootPromise = (async () => {
    const initialView: MailView = ui.view.kind === "outbox" ? { kind: "unified", role: "inbox" } : ui.view;
    const data = await api.bootstrap(initialView, BOOT_PAGE, STARTUP_SETTING_KEYS);
    ui.accounts = data.accounts;
    ui.folders = data.folders;
    ui.smartCounts = data.smartCounts;
    ui.outbox = data.outbox;
    ui.pendingAgentActions = data.pendingAgentActions;
    applyPrefs(data.settings);
    hydrateTheme(data.settings);
    hydrateBadgeMode(data.settings);
    hydrateShortcuts(data.settings);
    hydrateAi(data.settings);
    try {
      ui.trustedSenders = JSON.parse(data.settings["trusted_senders"] || "[]");
    } catch {
      ui.trustedSenders = [];
    }
    ui.list = data.messages;
    ui.hasMore = data.messages.length === BOOT_PAGE;
    ui.listReady = true;
    // Defer background prewarming and sent-recipients lookup until after the first interactive second.
    prewarmAround(ui.selectedId, 700);
    if (ui.autoTrustSent) {
      setTimeout(() => { loadSentRecipients(); }, 500);
    }
  })().catch((e) => {
    bootPromise = null;
    throw e;
  });
  return bootPromise;
}

export function setPaneWidths(side: number, list: number) {
  ui.sideWidth = Math.round(Math.max(170, Math.min(320, side)));
  ui.listWidth = Math.round(Math.max(260, Math.min(480, list)));
  api.setSetting("side_width", String(ui.sideWidth)).catch(() => {});
  api.setSetting("list_width", String(ui.listWidth)).catch(() => {});
}

export function toggleSidebar() {
  ui.sidebarCollapsed = !ui.sidebarCollapsed;
  api.setSetting("sidebar_collapsed", ui.sidebarCollapsed ? "on" : "off").catch(() => {});
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

export async function setAccountAlerts(accountId: number, notify: boolean, badgeMode: AccountBadgeMode) {
  const acc = ui.accounts.find((a) => a.id === accountId);
  if (acc) {
    acc.notify = notify;
    acc.badgeMode = badgeMode;
  }
  try {
    await api.setAccountAlerts(accountId, notify, badgeMode);
    await refreshBadge();
  } catch (e) {
    toast(String(e));
    await loadAccounts();
  }
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
  const [folders, counts] = await Promise.all([
    api.folders(),
    api.smartCounts().catch(() => ui.smartCounts),
  ]);
  ui.folders = folders;
  ui.smartCounts = counts;
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
  const hidden = hiddenPendingIds();
  const filtered = hidden.size ? rows.filter((r) => !hidden.has(r.id)) : rows;
  ui.list = reset ? filtered : [...ui.list, ...filtered];
  ui.listReady = true;
  if (q) ui.hasMore = false;
  // A conversation's row moves to its newest message when one arrives: follow it.
  if (ui.selectedId != null && !ui.list.some((m) => m.id === ui.selectedId)) {
    const holder = ui.list.find((m) => m.ids.includes(ui.selectedId!));
    if (holder) ui.selectedId = holder.id;
  }
  ui.selectedIds = ui.selectedIds.filter((id) => ui.list.some((m) => m.id === id));
  prewarmAround(ui.selectedId);
}

/** Views whose folders have no older mail left on the server. */
const reachedStart = new Set<string>();
const viewKey = (v: View) =>
  v.kind === "folder" ? `f${v.folderId}` : v.kind === "unified" ? `u${v.role}` : v.kind === "smart" ? `s${v.name}${v.accountId ?? ""}` : v.kind;

/** Next page from the local cache; past its end, older mail from the server. */
export async function loadMore() {
  if (ui.query.trim() || ui.loadingOlder || ui.view.kind === "outbox") return;
  if (ui.hasMore) {
    await loadList(false);
    return;
  }
  if (ui.view.kind === "smart") return;
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
  commitPending();
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

// ---------- background prewarming (local bodies only, never touches IMAP or remote images) ----------

const prewarmInFlight = new Set<number>();
let prewarmQueue: number[] = [];
let prewarmRunning = false;
let prewarmTimer: ReturnType<typeof setTimeout> | undefined;

/**
 * Prewarms a single message into the JS cache + inert prepared srcdoc string if its body is
 * already downloaded in SQLite. Never hits IMAP, never marks read, never fetches remote images.
 */
export async function prewarmMessage(id: number): Promise<void> {
  if (getCachedDetail(id) || prewarmInFlight.has(id)) return;
  prewarmInFlight.add(id);
  try {
    const [d, thread] = await Promise.all([
      api.cachedMessage(id).catch(() => null),
      getCachedThread(id) ? Promise.resolve(getCachedThread(id)!) : api.thread(id).catch(() => [] as Summary[]),
    ]);
    if (!d) return;
    putDetail(d);
    const t = thread.length ? thread : [d];
    putThread(id, t);
    // Pre-run DOMParser inertly so opening skips main-thread parsing.
    // Remote images are NOT preloaded on hover/background prewarm to save bandwidth and decoded bitmap RAM.
    const isPlain = !d.html;
    const body = d.html ?? textToHtml(d.text ?? "");
    const trusted = isSenderTrusted(d.fromAddr);
    const allowRemote = trusted || hasShownImages(id);
    const stripTrackers = shouldStripTrackers(d.fromAddr);
    cachedFrameDoc(id, body, allowRemote, isPlain, stripTrackers);
  } finally {
    prewarmInFlight.delete(id);
  }
}

async function drainPrewarm() {
  if (prewarmRunning) return;
  prewarmRunning = true;
  try {
    while (prewarmQueue.length) {
      const id = prewarmQueue.shift()!;
      if (!getCachedDetail(id)) {
        await prewarmMessage(id);
        await new Promise((r) => setTimeout(r, 15));
      }
    }
  } finally {
    prewarmRunning = false;
  }
}

/** Schedules low-priority prewarming for the rows around `selectedId` (or the top of the list). */
function prewarmAround(selectedId: number | null, delayMs = 40) {
  clearTimeout(prewarmTimer);
  prewarmTimer = setTimeout(() => {
    const list = ui.list;
    if (!list.length) return;
    const ids: number[] = [];
    const idx = selectedId != null ? list.findIndex((m) => m.id === selectedId) : -1;
    if (idx >= 0) {
      for (const offset of [1, -1, 2, 3, -2]) {
        const row = list[idx + offset];
        if (row && !getCachedDetail(row.id)) ids.push(row.id);
      }
    } else {
      for (let i = 0; i < Math.min(8, list.length); i++) {
        const row = list[i];
        if (row && !getCachedDetail(row.id)) ids.push(row.id);
      }
    }
    if (!ids.length) return;
    prewarmQueue = ids;
    drainPrewarm();
  }, delayMs);
}

function markRowReadIfNeeded(id: number, my: number) {
  const row = ui.list.find((m) => m.id === id);
  if (row && !row.seen && my === detailSeq) {
    row.seen = true;
    if (ui.detail) ui.detail.seen = true;
    for (const t of ui.thread) if (row.ids.includes(t.id)) t.seen = true;
    updateCachedSeen(row.ids, true);
    api.setSeen(row.ids, true).then(loadFolders).catch((e) => {
      toast(String(e));
      loadList(true);
      loadFolders();
    });
  }
}

function preloadAllowedImages(d: Detail) {
  if (!d.html) return;
  const role = roleOf(d);
  if (role === "junk" || role === "trash") return;
  if (isSenderTrusted(d.fromAddr) || hasShownImages(d.id)) {
    preloadRemoteImages(cachedRemoteImageUrls(d.id, d.html, shouldStripTrackers(d.fromAddr)));
  }
}

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

  const cached = peekCachedDetail(id);
  if (cached) {
    const cachedThread = getCachedThread(id);
    const t = cachedThread?.length ? cachedThread : [cached];
    const expandIds = [id, ...t.filter((m) => !m.seen).map((m) => m.id)];
    ui.detail = cached;
    ui.thread = t;
    ui.expandIds = expandIds;
    ui.details = cachedDetailsFor(t.map((m) => m.id));
    ui.detailLoading = false;
    upsertReaderPane(id, cached, t, expandIds);
    preloadAllowedImages(cached);
    markRowReadIfNeeded(id, my);
    prewarmAround(id);

    // Silently revalidate thread if it wasn't cached yet or its count changed.
    const row = ui.list.find((m) => m.id === id);
    if (!cachedThread || (row && row.count > cachedThread.length)) {
      api.thread(id).then((fresh) => {
        if (!fresh.length) return;
        putThread(id, fresh);
        updatePaneThread(id, fresh);
        if (my === detailSeq && ui.selectedId === id) {
          ui.thread = fresh;
          ui.details = cachedDetailsFor(fresh.map((m) => m.id));
        }
      }).catch(() => {});
    }

    // Drafts can be edited; silently revalidate in the background without flashing a spinner.
    if (roleOf(cached) === "drafts") {
      api.message(id).then((fresh) => {
        if (fresh.html !== cached.html || fresh.text !== cached.text || fresh.subject !== cached.subject) {
          putDetail(fresh);
          const curThread = getCachedThread(id) ?? [fresh];
          upsertReaderPane(id, fresh, curThread, [id]);
          if (my === detailSeq && ui.selectedId === id) {
            ui.detail = fresh;
            ui.details = { ...ui.details, [id]: fresh };
          }
        }
      }).catch(() => {});
    }
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
    putDetail(d);
    const t = thread.length ? thread : [d];
    putThread(id, t);
    const expandIds = [id, ...t.filter((m) => !m.seen).map((m) => m.id)];
    ui.details = cachedDetailsFor(t.map((m) => m.id));
    ui.detail = d;
    ui.expandIds = expandIds;
    ui.thread = t;
    upsertReaderPane(id, d, t, expandIds);
    preloadAllowedImages(d);
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
  markRowReadIfNeeded(id, my);
  prewarmAround(id);
}

/** Picks up new or changed messages in the open conversation. */
export async function refreshThread() {
  const id = ui.selectedId;
  if (id == null || ui.selectedIds.length > 1) return;
  const anchor = ui.detail?.id ?? id;
  try {
    const t = await api.thread(anchor);
    if (t.length) {
      putThread(anchor, t);
      updatePaneThread(anchor, t);
      if (id !== anchor) {
        putThread(id, t);
        updatePaneThread(id, t);
      }
      if (ui.selectedId === id) {
        ui.thread = t;
        ui.details = cachedDetailsFor(t.map((m) => m.id));
      }
    }
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

/** Takes rows off the list (opening the next row only if the currently open row was removed). Returns their message ids. */
function removeRows(rows: Summary[]): number[] {
  const gone = new Set(rows.map((r) => r.id));
  const first = ui.list.findIndex((m) => gone.has(m.id));
  const openRemoved = ui.selectedId != null && gone.has(ui.selectedId);
  ui.list = ui.list.filter((m) => !gone.has(m.id));
  ui.selectedIds = ui.selectedIds.filter((id) => !gone.has(id));
  if (openRemoved) {
    const next = first < 0 ? null : (ui.list[first] ?? ui.list[first - 1] ?? null);
    openMessage(next ? next.id : null);
  }
  return idsOf(rows);
}

// ---------- actions & deferred undo ----------

type PendingUndo = {
  rowIds: Set<number>;
  snapshot: { row: Summary; index: number }[];
  unreadDeltas: Map<number, number>;
  prevSelectedId: number | null;
  prevSelectedIds: number[];
  timer: ReturnType<typeof setTimeout>;
  commit: () => void;
};

let pendingUndo: PendingUndo | null = null;
const committingIds = new Set<number>();

function hiddenPendingIds(): Set<number> {
  if (!pendingUndo && !committingIds.size) return committingIds;
  const s = new Set<number>(committingIds);
  if (pendingUndo) for (const id of pendingUndo.rowIds) s.add(id);
  return s;
}

/** Immediately commits any pending undoable action (called before another action, view change, or window close). */
export function commitPending() {
  if (!pendingUndo) return;
  const p = pendingUndo;
  pendingUndo = null;
  clearTimeout(p.timer);
  p.commit();
}

function revertPending() {
  if (!pendingUndo) return;
  const p = pendingUndo;
  pendingUndo = null;
  clearTimeout(p.timer);
  // Restore rows into ui.list at their original positions.
  const next = [...ui.list];
  const ordered = [...p.snapshot].sort((a, b) => a.index - b.index);
  for (const { row, index } of ordered) {
    if (!next.some((m) => m.id === row.id)) {
      next.splice(Math.min(index, next.length), 0, row);
    }
  }
  ui.list = next;
  for (const [fid, delta] of p.unreadDeltas) {
    const f = folderById(fid);
    if (f) f.unread += delta;
  }
  if (p.prevSelectedIds.length > 1) {
    ui.selectedIds = p.prevSelectedIds;
  } else if (p.prevSelectedId != null && p.rowIds.has(p.prevSelectedId)) {
    openMessage(p.prevSelectedId);
  }
  toast("Undone", { ms: 1600 });
}

let toastTimer: ReturnType<typeof setTimeout> | undefined;
export function toast(text: string, opts: { undo?: () => void; action?: { label: string; fn: () => void }; ms?: number } = {}) {
  ui.toast = { text, undo: opts.undo, action: opts.action };
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (ui.toast = null), opts.ms ?? 3200);
}

export async function loadPendingAgentActions() {
  try {
    ui.pendingAgentActions = await api.pendingAgentActionsCount();
  } catch {
    ui.pendingAgentActions = 0;
  }
}

export async function forwardAsAttachment(d: Detail) {
  try {
    const file = await api.stageEml(d.id);
    compose("new", {
      accountId: d.accountId,
      subject: d.subject ? `Fwd: ${d.subject}` : "Forwarded message",
      attachments: [file],
    });
  } catch (e) {
    toast(String(e));
  }
}

export async function openRawSource(id: number, subject = "") {
  try {
    const raw = await api.rawSource(id);
    ui.sourceView = { id, subject, raw };
  } catch (e) {
    toast(String(e));
  }
}

export async function exportMessageEml(id: number, subject = "") {
  try {
    const { save } = await import("@tauri-apps/plugin-dialog");
    const stem = (subject || `message-${id}`).replace(/[/:*?"<>|\\]+/g, "_").trim() || `message-${id}`;
    const dest = await save({
      defaultPath: `${stem}.eml`,
      filters: [{ name: "Email", extensions: ["eml"] }],
    });
    if (!dest) return;
    await api.exportEml(id, dest);
    toast("Exported .eml");
  } catch (e) {
    toast(String(e));
  }
}

export async function printOpenThread() {
  const d = ui.detail;
  if (!d || ui.selectedIds.length > 1) return;
  const thread = ui.thread.length ? ui.thread : [d];
  try {
    const items = await Promise.all(
      thread.map(async (m) => {
        const hit = getCachedDetail(m.id) ?? ui.details[m.id];
        if (hit) return hit;
        const fetched = await api.message(m.id);
        putDetail(fetched);
        return fetched;
      }),
    );
    const { printMessages } = await import("./print");
    printMessages(d.subject, items);
  } catch (e) {
    toast(`Couldn't print: ${e}`);
  }
}

export function openMailto(url: string) {
  import("./mailto").then(({ parseMailto }) => {
    const parsed = parseMailto(url);
    if (!parsed) return;
    compose("new", {
      to: parsed.to,
      cc: parsed.cc,
      bcc: parsed.bcc,
      subject: parsed.subject,
      bodyText: parsed.body,
    });
  });
}

/** Copies the open conversation as plain text (for an AI chat, notes, a ticket). */
export async function copyThread() {
  const d = ui.detail;
  if (!d || ui.selectedIds.length > 1) return;
  const thread = ui.thread.length ? ui.thread : [d];
  try {
    const known = { ...ui.details, ...cachedDetailsFor(thread.map((m) => m.id)) };
    await api.copyText(await threadText(d.subject, thread, known));
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

/** Runs the toast's primary action button and closes it. */
export function runToastAction() {
  const fn = ui.toast?.action?.fn;
  dismissToast();
  fn?.();
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

/** Removes rows optimistically with a 4.5s Undo window before committing to the backend. */
function deferBulk(label: string, rows: Summary[], call: (ids: number[]) => Promise<unknown>) {
  if (!rows.length) return;
  commitPending();
  const rowIds = new Set(rows.map((r) => r.id));
  const snapshot = rows
    .map((row) => ({ row, index: ui.list.findIndex((m) => m.id === row.id) }))
    .filter((x) => x.index >= 0);
  const unreadDeltas = new Map<number, number>();
  for (const r of rows) {
    if (!r.seen) {
      unreadDeltas.set(r.folderId, (unreadDeltas.get(r.folderId) ?? 0) + 1);
      const f = folderById(r.folderId);
      if (f && f.unread > 0) f.unread = Math.max(0, f.unread - 1);
    }
  }
  const prevSelectedId = ui.selectedId;
  const prevSelectedIds = [...ui.selectedIds];
  const ids = removeRows(rows);

  const commit = () => {
    for (const id of rowIds) committingIds.add(id);
    evictMessages(ids);
    run(call(ids)).finally(() => {
      for (const id of rowIds) committingIds.delete(id);
    });
  };
  const ms = 4500;
  const timer = setTimeout(() => {
    if (pendingUndo?.timer === timer) {
      pendingUndo = null;
      commit();
    }
  }, ms);
  pendingUndo = { rowIds, snapshot, unreadDeltas, prevSelectedId, prevSelectedIds, timer, commit };
  toast(label, { undo: revertPending, ms });
}

/** Checks the server for new mail in just this folder (or unified folder). */
export function checkForMail(view: MailView) {
  api.syncView(view).catch((e) => toast(String(e)));
}

/** Checks the server for new mail in every folder of one account. */
export function checkAccountForMail(accountId: number) {
  api.syncAccount(accountId).catch((e) => toast(String(e)));
}

export const archiveRows = (rows: Summary[]) => deferBulk(counted("Archived", rows.length), rows, api.archive);
export const trashRows = (rows: Summary[]) => deferBulk(counted("Deleted", rows.length), rows, api.trash);
export const archiveSelected = () => archiveRows(selectedRows());
export const trashSelected = () => trashRows(selectedRows());

export async function permanentDeleteRows(rows: Summary[]) {
  if (!rows.length) return;
  const { ask } = await import("@tauri-apps/plugin-dialog");
  const n = rows.length;
  const ok = await ask(
    n === 1 ? "Permanently delete this message? This can't be undone." : `Permanently delete ${n} conversations? This can't be undone.`,
    { title: "Delete Permanently", kind: "warning", okLabel: "Delete", cancelLabel: "Cancel" },
  );
  if (!ok) return;
  commitPending();
  const ids = removeRows(rows);
  evictMessages(ids);
  await run(api.permanentDelete(ids));
  toast(counted("Permanently deleted", n));
}

export const permanentDeleteSelected = () => permanentDeleteRows(selectedRows());

export async function emptyFolderView(view: MailView) {
  const { ask } = await import("@tauri-apps/plugin-dialog");
  const label =
    view.kind === "unified"
      ? view.role === "junk" ? "All Junk" : "All Trash"
      : view.kind === "folder"
        ? (folderById(view.folderId)?.name ?? "this folder")
        : "this folder";
  const ok = await ask(`Permanently delete all messages in ${label}? This can't be undone.`, {
    title: `Empty ${label}`,
    kind: "warning",
    okLabel: "Empty",
    cancelLabel: "Cancel",
  });
  if (!ok) return;
  commitPending();
  await run(api.emptyFolder(view));
  loadList(true);
  toast(`Emptied ${label}`);
}

export function promptCreateFolder(accountId: number, parentId: number | null = null, parentName?: string) {
  ui.folderPrompt = { kind: "create", accountId, parentId, parentName };
}

export function promptRenameFolder(folder: Folder) {
  ui.folderPrompt = { kind: "rename", folder };
}

export async function submitFolderPrompt(name: string) {
  const p = ui.folderPrompt;
  if (!p) return;
  try {
    if (p.kind === "create") {
      await api.createFolder(p.accountId, name, p.parentId);
      await loadFolders();
      toast(`Created folder “${name}”`);
    } else {
      await api.renameFolder(p.folder.id, name);
      await loadFolders();
      toast(`Renamed to “${name}”`);
    }
  } catch (e) {
    toast(String(e));
    throw e;
  }
}

export async function deleteFolderConfirm(folder: Folder) {
  const { ask } = await import("@tauri-apps/plugin-dialog");
  const ok = await ask(`Delete folder “${folder.name}” and its messages from the server? This can't be undone.`, {
    title: "Delete Folder",
    kind: "warning",
    okLabel: "Delete",
    cancelLabel: "Cancel",
  });
  if (!ok) return;
  try {
    await api.deleteFolder(folder.id);
    if (ui.view.kind === "folder" && ui.view.folderId === folder.id) {
      setView({ kind: "unified", role: "inbox" });
    }
    await loadFolders();
    toast(`Deleted folder “${folder.name}”`);
  } catch (e) {
    toast(String(e));
  }
}

/** True when the selection sits in a Junk folder, so the junk action means "Not Junk". */
export function selectedIsJunk(): boolean {
  return roleOf(selectedRows()[0]) === "junk";
}

/** Moves the selection to Junk, or back to the Inbox if it's already in Junk. */
export function toggleJunk() {
  const rows = selectedRows();
  if (selectedIsJunk()) deferBulk(counted("Moved to Inbox", rows.length), rows, api.notJunk);
  else deferBulk(counted("Moved to Junk", rows.length), rows, api.junk);
}

/** Read / unread for arbitrary rows (used by toolbar and row hover quick-actions). */
export function toggleSeenRows(rows: Summary[]) {
  if (!rows.length) return;
  const seen = rows.some((r) => !r.seen);
  for (const r of rows) {
    if (r.seen !== seen) {
      r.seen = seen;
      const f = folderById(r.folderId);
      if (f) f.unread = Math.max(0, f.unread + (seen ? -1 : 1));
    }
  }
  const ids = seen ? idsOf(rows) : rows.map((r) => r.id);
  for (const t of ui.thread) if (ids.includes(t.id)) t.seen = seen;
  if (ui.detail && ids.includes(ui.detail.id)) ui.detail.seen = seen;
  updateCachedSeen(ids, seen);
  run(api.setSeen(ids, seen));
}

export const toggleSeen = () => toggleSeenRows(selectedRows());

/** Flags the newest message of each conversation; unflagging clears every message. */
export function toggleFlag() {
  const rows = selectedRows();
  if (!rows.length) return;
  const flagged = rows.some((r) => !r.flagged);
  for (const r of rows) r.flagged = flagged;
  const ids = flagged ? rows.map((r) => r.id) : idsOf(rows);
  for (const t of ui.thread) if (ids.includes(t.id)) t.flagged = flagged;
  if (ui.detail && ids.includes(ui.detail.id)) ui.detail.flagged = flagged;
  updateCachedFlagged(ids, flagged);
  run(api.setFlagged(ids, flagged));
}

/** Marks everything in a folder (or a unified/smart view) as read, here and on the server. */
export async function markAllRead(view: MailView) {
  if (view.kind === "smart") {
    for (const m of ui.list) m.seen = true;
    if (ui.detail) ui.detail.seen = true;
    await run(api.markRead(view));
    if (ui.view.kind === "smart" && ui.view.name === "unread") loadList(true);
    return;
  }
  const inView = (fid: number) =>
    view.kind === "folder" ? fid === view.folderId : view.kind === "unified" && folderById(fid)?.role === view.role;
  for (const m of ui.list) if (inView(m.folderId)) m.seen = true;
  if (ui.detail && inView(ui.detail.folderId)) ui.detail.seen = true;
  markCachedFolderRead(inView);
  for (const f of ui.folders) if (inView(f.id)) f.unread = 0;
  await run(api.markRead(view));
}

export function snoozeTargetRows(rows: Summary[]) {
  if (rows.length) ui.picker = { kind: "snooze", rows };
}

export function snoozeSelected() {
  snoozeTargetRows(selectedRows());
}

export function snoozeRows(rows: Summary[], until: Date) {
  const gone = new Set(rows.map((r) => r.id));
  const shown = ui.list.filter((m) => gone.has(m.id));
  const target = shown.length ? shown : rows;
  deferBulk(`Snoozed until ${whenLabel(until)}`, target, (ids) => api.snooze(ids, toUnix(until)));
}

export const unsnoozeRows = (rows: Summary[]) => deferBulk(counted("Unsnoozed", rows.length), rows, api.unsnooze);

export function unsnoozeSelected() {
  unsnoozeRows(selectedRows());
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
  const target = shown.length ? shown : rows;
  deferBulk(`Moved to ${folder.name}`, target, (ids) => api.moveTo(ids, folder.id));
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
  const draftMid = extra.restore?.message.messageId;
  if (draftMid) {
    const existing = ui.composers.find((c) => c.restore?.message.messageId === draftMid);
    if (existing) {
      existing.minimized = false;
      ui.activeComposerKey = existing.key;
      return;
    }
  }
  // Minimize older docked composers if opening a 3rd+ draft so the screen stays tidy.
  const openDocked = ui.composers.filter((c) => !c.minimized && !c.expanded);
  if (openDocked.length >= 2) {
    openDocked[0].minimized = true;
  }
  const key = ++composeKey;
  ui.composers = [...ui.composers, { mode, source, key, minimized: false, expanded: false, ...extra }];
  ui.activeComposerKey = key;
}

export function closeComposer(key: number) {
  ui.composers = ui.composers.filter((c) => c.key !== key);
  if (ui.activeComposerKey === key) {
    const open = ui.composers.filter((c) => !c.minimized);
    const next = open[open.length - 1] ?? ui.composers[ui.composers.length - 1] ?? null;
    ui.activeComposerKey = next ? next.key : null;
  }
}

export function focusComposer(key: number) {
  ui.activeComposerKey = key;
  const c = ui.composers.find((x) => x.key === key);
  if (c && c.minimized) c.minimized = false;
}

export function toggleMinimizeComposer(key: number) {
  const c = ui.composers.find((x) => x.key === key);
  if (!c) return;
  c.minimized = !c.minimized;
  if (c.minimized) {
    c.expanded = false;
    if (ui.activeComposerKey === key) {
      const open = ui.composers.filter((x) => x.key !== key && !x.minimized);
      const next = open[open.length - 1] ?? null;
      ui.activeComposerKey = next ? next.key : null;
    }
  } else {
    ui.activeComposerKey = key;
  }
}

export function toggleExpandComposer(key: number) {
  const c = ui.composers.find((x) => x.key === key);
  if (!c) return;
  c.expanded = !c.expanded;
  if (c.expanded) {
    c.minimized = false;
    ui.activeComposerKey = key;
  }
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
        fromEmail: d.fromAddr || null,
        fromName: d.fromName || null,
        replyTo: d.replyTo.length ? formatAddr(d.replyTo[0]) : "",
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

export async function setRemoteImages(mode: RemoteImagesMode) {
  ui.remoteImages = mode;
  await api.setSetting("remote_images", mode);
}

export async function setAutoTrustSent(on: boolean) {
  ui.autoTrustSent = on;
  await api.setSetting("auto_trust_sent", on ? "on" : "off");
  if (on && ui.sentRecipients.size === 0) loadSentRecipients();
}

export async function loadSentRecipients() {
  try {
    const list = await api.sentRecipients();
    ui.sentRecipients = new Set(list);
  } catch {
    // keep existing set
  }
}

export function noteSentRecipients(addrs: string[]) {
  let next: Set<string> | null = null;
  for (const raw of addrs) {
    const e = extractEmailAddress(raw);
    if (e && !ui.sentRecipients.has(e)) {
      if (!next) next = new Set(ui.sentRecipients);
      next.add(e);
    }
  }
  if (next) ui.sentRecipients = next;
}

const ownEmails = $derived(new Set(ui.accounts.map((a) => a.email.toLowerCase())));

/** Lowercased set of the user's own account email addresses (reactive). */
export function myEmails(): Set<string> {
  return ownEmails;
}

function isExplicitlyTrusted(email: string): boolean {
  return isRemoteImageAllowed(email, {
    always: false,
    entries: ui.trustedSenders,
    autoTrustSent: ui.autoTrustSent,
    sentRecipients: ui.sentRecipients,
    ownEmails,
  });
}

export function isSenderTrusted(email: string): boolean {
  if (ui.remoteImages === "always" || ui.remoteImages === "strip_trackers") return true;
  return isExplicitlyTrusted(email);
}

export function shouldStripTrackers(email: string): boolean {
  return ui.remoteImages === "strip_trackers" && !isExplicitlyTrusted(email);
}

export async function loadTrusted() {
  try {
    ui.trustedSenders = JSON.parse((await api.getSetting("trusted_senders")) || "[]");
  } catch {
    ui.trustedSenders = [];
  }
  if (ui.autoTrustSent) await loadSentRecipients();
}

export async function trustSender(entry: string): Promise<TrustEntryResult> {
  const res = validateTrustEntry(entry);
  if (!res.ok) return res;
  if (!ui.trustedSenders.includes(res.entry)) {
    ui.trustedSenders = [...ui.trustedSenders, res.entry];
    await api.setSetting("trusted_senders", JSON.stringify(ui.trustedSenders));
  }
  return res;
}

export async function trustDomain(domain: string): Promise<TrustEntryResult> {
  return trustSender(`@${domain.trim().replace(/^@+/, "")}`);
}

export async function untrustSender(entry: string) {
  const e = entry.trim().toLowerCase();
  ui.trustedSenders = ui.trustedSenders.filter((x) => x.toLowerCase() !== e);
  await api.setSetting("trusted_senders", JSON.stringify(ui.trustedSenders));
}
