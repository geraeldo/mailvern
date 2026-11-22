import { api, type Account, type Detail, type Folder, type Summary, type View } from "./api";

export type ComposeMode = "new" | "reply" | "replyAll" | "forward";
export type ComposeInit = { mode: ComposeMode; source?: Detail; accountId?: number; to?: string[]; key: number };
export type SyncStatus = { state: "syncing" | "idle" | "error"; message: string | null };

const PAGE = 200;

export const ui = $state({
  accounts: [] as Account[],
  folders: [] as Folder[],
  view: { kind: "unified" } as View,
  list: [] as Summary[],
  hasMore: false,
  listReady: false,
  selectedId: null as number | null,
  detail: null as Detail | null,
  detailLoading: false,
  detailError: null as string | null,
  query: "",
  compose: null as ComposeInit | null,
  settings: false,
  status: {} as Record<number, SyncStatus>,
  toast: null as { text: string; undo?: () => void } | null,
  trustedSenders: [] as string[],
});

export function accountById(id: number): Account | undefined {
  return ui.accounts.find((a) => a.id === id);
}

export function folderById(id: number): Folder | undefined {
  return ui.folders.find((f) => f.id === id);
}

export function viewTitle(): string {
  if (ui.query.trim()) return "Search";
  if (ui.view.kind === "unified") return "Inbox";
  if (ui.view.kind === "folder") {
    const id = ui.view.folderId;
    return folderById(id)?.name ?? "";
  }
  return "";
}

export async function loadAccounts() {
  ui.accounts = await api.accounts();
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

export function setView(v: View) {
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

export function archiveSelected() {
  const id = removeSelected();
  if (id != null) run(api.archive([id])).then(() => toast("Archived"));
}

export function trashSelected() {
  const id = removeSelected();
  if (id != null) run(api.trash([id])).then(() => toast("Deleted"));
}

export function toggleSeen() {
  const row = ui.list.find((m) => m.id === ui.selectedId);
  if (!row) return;
  row.seen = !row.seen;
  run(api.setSeen([row.id], row.seen));
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
    ui.settings = true;
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
