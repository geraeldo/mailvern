import { api, type Account, type Detail, type Folder, type Summary, type View } from "./api";

export type ComposeMode = "new" | "reply" | "replyAll" | "forward";
export type ComposeInit = { mode: ComposeMode; source?: Detail; accountId?: number; to?: string[]; key: number };
export type SettingsSection = "shortcuts";
export type SyncStatus = { state: "syncing" | "idle" | "error"; message: string | null };

const PAGE = 200;

export const ui = $state({
  accounts: [] as Account[],
  folders: [] as Folder[],
  view: { kind: "unified", role: "inbox" } as View,
  list: [] as Summary[],
  hasMore: false,
  listReady: false,
  loadingOlder: false,
  selectedId: null as number | null,
  detail: null as Detail | null,
  detailLoading: false,
  detailError: null as string | null,
  query: "",
  compose: null as ComposeInit | null,
  /** App settings (shortcuts and, later, other preferences). */
  settings: null as SettingsSection | null,
  /** Account settings: the account being edited, or "new". */
  accountSettings: null as number | "new" | null,
  status: {} as Record<number, SyncStatus>,
  toast: null as { text: string; undo?: () => void } | null,
  trustedSenders: [] as string[],
});

export function openSettings(section: SettingsSection = "shortcuts") {
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
  { role: "drafts", name: "Drafts" },
  { role: "sent", name: "Sent" },
  { role: "archive", name: "Archive" },
  { role: "junk", name: "Junk" },
  { role: "trash", name: "Trash" },
];

export function viewTitle(): string {
  if (ui.query.trim()) return "Search";
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

export async function loadAccounts() {
  ui.accounts = await api.accounts();
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

let listSeq = 0;
export async function loadList(reset = true) {
  const my = ++listSeq;
  const q = ui.query.trim();
  let rows: Summary[];
  if (q) {
    if (!reset) return;
    rows = await api.search(q);
  } else {
    const limit = reset ? Math.max(PAGE, ui.list.length) : PAGE;
    rows = await api.messages(ui.view, null, reset ? 0 : ui.list.length, limit);
    if (my === listSeq) ui.hasMore = rows.length === limit;
  }
  if (my !== listSeq) return;
  ui.list = reset ? rows : [...ui.list, ...rows];
  ui.listReady = true;
  if (q) ui.hasMore = false;
}

/** Views whose folders have no older mail left on the server. */
const reachedStart = new Set<string>();
const viewKey = (v: View) => (v.kind === "folder" ? `f${v.folderId}` : v.kind === "unified" ? `u${v.role}` : v.kind);

/** Next page from the local cache; past its end, older mail from the server. */
export async function loadMore() {
  if (ui.query.trim() || ui.loadingOlder) return;
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

export function setView(v: View) {
  reachedStart.delete(viewKey(v));
  ui.view = v;
  ui.query = "";
  ui.list = [];
  ui.listReady = false;
  ui.selectedId = null;
  ui.detail = null;
  loadList(true);
}

export function setQuery(q: string) {
  ui.query = q;
  ui.list = [];
  ui.selectedId = null;
  ui.detail = null;
  loadList(true);
}

let detailSeq = 0;
export async function openMessage(id: number | null) {
  ui.selectedId = id;
  const my = ++detailSeq;
  ui.detailError = null;
  if (id == null) {
    ui.detail = null;
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
    const d = await api.message(id);
    if (my !== detailSeq) return;
    ui.detail = d;
  } catch (e) {
    if (my === detailSeq) {
      ui.detail = null;
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
    api.setSeen([id], true).then(loadFolders);
  }
}

export function move(delta: number) {
  if (!ui.list.length) return;
  const i = ui.list.findIndex((m) => m.id === ui.selectedId);
  const next = i < 0 ? 0 : Math.min(ui.list.length - 1, Math.max(0, i + delta));
  openMessage(ui.list[next].id);
}

function removeSelected(): number | null {
  const id = ui.selectedId;
  if (id == null) return null;
  const i = ui.list.findIndex((m) => m.id === id);
  ui.list = ui.list.filter((m) => m.id !== id);
  const next = ui.list[i] ?? ui.list[i - 1] ?? null;
  openMessage(next ? next.id : null);
  return id;
}

let toastTimer: ReturnType<typeof setTimeout> | undefined;
export function toast(text: string) {
  ui.toast = { text };
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (ui.toast = null), 3200);
}

async function run(p: Promise<unknown>) {
  try {
    await p;
  } catch (e) {
    toast(String(e));
    loadList(true);
  }
  loadFolders();
}

/** Checks the server for new mail in just this folder (or unified folder). */
export function checkForMail(view: View) {
  api.syncView(view).catch((e) => toast(String(e)));
}

/** Checks the server for new mail in every folder of one account. */
export function checkAccountForMail(accountId: number) {
  api.syncAccount(accountId).catch((e) => toast(String(e)));
}

export function archiveSelected() {
  const id = removeSelected();
  if (id != null) run(api.archive([id])).then(() => toast("Archived"));
}

export function trashSelected() {
  const id = removeSelected();
  if (id != null) run(api.trash([id])).then(() => toast("Deleted"));
}

/** True when the open message sits in a Junk folder, so the junk action means "Not Junk". */
export function selectedIsJunk(): boolean {
  const row = ui.list.find((m) => m.id === ui.selectedId);
  const fid = row?.folderId ?? ui.detail?.folderId;
  return fid != null && folderById(fid)?.role === "junk";
}

/** Moves the open message to Junk, or back to the Inbox if it's already in Junk. */
export function toggleJunk() {
  const notJunk = selectedIsJunk();
  const id = removeSelected();
  if (id == null) return;
  if (notJunk) run(api.notJunk([id])).then(() => toast("Moved to Inbox"));
  else run(api.junk([id])).then(() => toast("Moved to Junk"));
}

export function toggleSeen() {
  const row = ui.list.find((m) => m.id === ui.selectedId);
  if (!row) return;
  row.seen = !row.seen;
  run(api.setSeen([row.id], row.seen));
}

/** Marks everything in a folder (or a unified folder) as read, here and on the server. */
export async function markAllRead(view: View) {
  const inView = (fid: number) =>
    view.kind === "folder" ? fid === view.folderId : view.kind === "unified" && folderById(fid)?.role === view.role;
  for (const m of ui.list) if (inView(m.folderId)) m.seen = true;
  if (ui.detail && inView(ui.detail.folderId)) ui.detail.seen = true;
  for (const f of ui.folders) if (inView(f.id)) f.unread = 0;
  await run(api.markRead(view));
}

export function toggleFlag() {
  const row = ui.list.find((m) => m.id === ui.selectedId);
  if (!row) return;
  row.flagged = !row.flagged;
  if (ui.detail) ui.detail.flagged = row.flagged;
  run(api.setFlagged([row.id], row.flagged));
}

let composeKey = 0;
export function compose(mode: ComposeMode = "new", extra: Partial<ComposeInit> = {}) {
  if (!ui.accounts.length) {
    openAccountSettings("new");
    return;
  }
  const source = mode === "new" ? undefined : (ui.detail ?? undefined);
  if (mode !== "new" && !source) return;
  ui.compose = { mode, source, key: ++composeKey, ...extra };
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
