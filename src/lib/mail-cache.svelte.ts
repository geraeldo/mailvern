import { SvelteMap, SvelteSet } from "svelte/reactivity";
import type { Detail, Summary } from "./api";
import { frameDoc } from "./html";

/** Max parsed message bodies kept in memory. */
const MAX_DETAILS = 60;
/** Soft byte budget for cached HTML/text bodies (40 MB). */
const MAX_DETAIL_BYTES = 40 * 1024 * 1024;
/** Max conversation thread lists kept in memory. */
const MAX_THREADS = 80;
/** Max rendered reader panes kept mounted (hidden) so switching back is instant. */
const MAX_PANES = 8;
/** Max prepared iframe srcdoc strings cached. */
const MAX_DOCS = 40;
/** Max remembered iframe heights. */
const MAX_HEIGHTS = 120;

export type ReaderPane = {
  id: number;
  detail: Detail;
  thread: Summary[];
  expandIds: number[];
  scrollTop: number;
  lastUsed: number;
};

let paneTick = 0;

/** Reactive message body cache, keyed by message id. */
const details = new SvelteMap<number, Detail>();
const detailSizes = new Map<number, number>();
let detailBytes = 0;

/** Thread summaries cache, keyed by anchor message id. */
const threads = new Map<number, Summary[]>();

/** Messages where the user clicked "Show once" for remote images. */
const shownImages = new SvelteSet<number>();

/** Prepared srcdoc strings, keyed by `${id}:${allowRemote ? 1 : 0}:${plain ? 1 : 0}`. */
const docs = new Map<string, string>();

/** Last measured iframe height per message id. */
const heights = new Map<number, number>();

/** Keep-alive pool of recently opened reader panes (oldest first, most recent last). */
export const readerPool = $state<{ panes: ReaderPane[] }>({ panes: [] });

function byteSize(d: Detail): number {
  return (d.html?.length ?? 0) + (d.text?.length ?? 0);
}

function touchMap<K, V>(map: Map<K, V>, key: K, value: V) {
  map.delete(key);
  map.set(key, value);
}

function pinnedMessageIds(): Set<number> {
  const pinned = new Set<number>();
  for (const p of readerPool.panes) {
    pinned.add(p.id);
    for (const m of p.thread) pinned.add(m.id);
  }
  return pinned;
}

function evictDetails() {
  if (details.size <= MAX_DETAILS && detailBytes <= MAX_DETAIL_BYTES) return;
  const pinned = pinnedMessageIds();
  for (const id of details.keys()) {
    if (details.size <= MAX_DETAILS && detailBytes <= MAX_DETAIL_BYTES) break;
    if (pinned.has(id)) continue;
    details.delete(id);
    detailBytes -= detailSizes.get(id) ?? 0;
    detailSizes.delete(id);
    evictDocsFor(id);
  }
}

function evictDocsFor(id: number) {
  const prefix = `${id}:`;
  for (const k of docs.keys()) {
    if (k.startsWith(prefix)) docs.delete(k);
  }
}

/** Reactive read of a cached message detail. */
export function getCachedDetail(id: number): Detail | undefined {
  return details.get(id);
}

/** Non-reactive check + LRU touch for opening a message. */
export function peekCachedDetail(id: number): Detail | undefined {
  const d = details.get(id);
  if (d) {
    details.delete(id);
    details.set(id, d);
  }
  return d;
}

/** Stores a message detail in the bounded LRU cache. */
export function putDetail(d: Detail) {
  const prevSize = detailSizes.get(d.id) ?? 0;
  const nextSize = byteSize(d);
  detailBytes = Math.max(0, detailBytes - prevSize) + nextSize;
  detailSizes.set(d.id, nextSize);
  details.delete(d.id);
  details.set(d.id, d);
  // Invalidate prepared docs if this message was re-fetched (e.g. edited draft).
  evictDocsFor(d.id);
  evictDetails();
}

export function getCachedThread(id: number): Summary[] | undefined {
  const t = threads.get(id);
  if (t) touchMap(threads, id, t);
  return t;
}

export function putThread(id: number, thread: Summary[]) {
  touchMap(threads, id, thread);
  while (threads.size > MAX_THREADS) {
    const oldest = threads.keys().next().value;
    if (oldest === undefined) break;
    threads.delete(oldest);
  }
}

/** Returns a map of cached details for the given message ids (used by copyThread). */
export function cachedDetailsFor(ids: number[]): Record<number, Detail> {
  const out: Record<number, Detail> = {};
  for (const id of ids) {
    const d = details.get(id);
    if (d) out[id] = d;
  }
  return out;
}

/**
 * Adds or updates a conversation pane in the keep-alive DOM pool.
 * Existing panes keep their array slot so Svelte never moves their DOM nodes
 * (moving an <iframe> in WebKit destroys and reloads its browsing context).
 */
export function upsertReaderPane(id: number, detail: Detail, thread: Summary[], expandIds: number[]): ReaderPane {
  const existing = readerPool.panes.find((p) => p.id === id);
  if (existing) {
    existing.detail = detail;
    existing.thread = thread;
    existing.lastUsed = ++paneTick;
    // Preserve any newly-added expandIds without collapsing what the user opened.
    if (expandIds.length) {
      const merged = new Set([...existing.expandIds, ...expandIds]);
      existing.expandIds = [...merged];
    }
    return existing;
  }
  while (readerPool.panes.length >= MAX_PANES) {
    let oldestIdx = 0;
    for (let i = 1; i < readerPool.panes.length; i++) {
      if (readerPool.panes[i].lastUsed < readerPool.panes[oldestIdx].lastUsed) oldestIdx = i;
    }
    readerPool.panes.splice(oldestIdx, 1);
  }
  const pane: ReaderPane = { id, detail, thread, expandIds, scrollTop: 0, lastUsed: ++paneTick };
  readerPool.panes.push(pane);
  evictDetails();
  return pane;
}

/** Updates a pane's thread list in-place if it is currently in the keep-alive pool. */
export function updatePaneThread(id: number, thread: Summary[]) {
  const pane = readerPool.panes.find((p) => p.id === id);
  if (pane) pane.thread = thread;
}

/** Whether the user clicked "Show once" for this message's remote images. */
export function hasShownImages(id: number): boolean {
  return shownImages.has(id);
}

export function markShownImages(id: number) {
  shownImages.add(id);
}

/** Cached `frameDoc` builder to avoid repeated DOMParser passes on the same message. */
export function cachedFrameDoc(cacheKey: number | undefined, html: string, allowRemote: boolean, plain: boolean): string {
  if (cacheKey == null) return frameDoc(html, allowRemote, plain);
  const key = `${cacheKey}:${allowRemote ? 1 : 0}:${plain ? 1 : 0}`;
  const hit = docs.get(key);
  if (hit !== undefined) {
    touchMap(docs, key, hit);
    return hit;
  }
  const built = frameDoc(html, allowRemote, plain);
  touchMap(docs, key, built);
  while (docs.size > MAX_DOCS) {
    const oldest = docs.keys().next().value;
    if (oldest === undefined) break;
    docs.delete(oldest);
  }
  return built;
}

export function getCachedFrameHeight(cacheKey: number | undefined): number | undefined {
  if (cacheKey == null) return undefined;
  return heights.get(cacheKey);
}

export function setCachedFrameHeight(cacheKey: number | undefined, px: number) {
  if (cacheKey == null || px <= 0) return;
  touchMap(heights, cacheKey, px);
  while (heights.size > MAX_HEIGHTS) {
    const oldest = heights.keys().next().value;
    if (oldest === undefined) break;
    heights.delete(oldest);
  }
}

// ---------- cache coherence ----------

export function updateCachedSeen(ids: number[], seen: boolean) {
  const set = new Set(ids);
  for (const id of set) {
    const d = details.get(id);
    if (d && d.seen !== seen) details.set(id, { ...d, seen });
  }
  for (const [k, list] of threads) {
    if (list.some((m) => set.has(m.id) && m.seen !== seen)) {
      threads.set(
        k,
        list.map((m) => (set.has(m.id) ? { ...m, seen } : m)),
      );
    }
  }
  for (const p of readerPool.panes) {
    if (set.has(p.detail.id) && p.detail.seen !== seen) p.detail = { ...p.detail, seen };
    if (p.thread.some((m) => set.has(m.id) && m.seen !== seen)) {
      p.thread = p.thread.map((m) => (set.has(m.id) ? { ...m, seen } : m));
    }
  }
}

export function updateCachedFlagged(ids: number[], flagged: boolean) {
  const set = new Set(ids);
  for (const id of set) {
    const d = details.get(id);
    if (d && d.flagged !== flagged) details.set(id, { ...d, flagged });
  }
  for (const [k, list] of threads) {
    if (list.some((m) => set.has(m.id) && m.flagged !== flagged)) {
      threads.set(
        k,
        list.map((m) => (set.has(m.id) ? { ...m, flagged } : m)),
      );
    }
  }
  for (const p of readerPool.panes) {
    if (set.has(p.detail.id) && p.detail.flagged !== flagged) p.detail = { ...p.detail, flagged };
    if (p.thread.some((m) => set.has(m.id) && m.flagged !== flagged)) {
      p.thread = p.thread.map((m) => (set.has(m.id) ? { ...m, flagged } : m));
    }
  }
}

export function markCachedFolderRead(inView: (folderId: number) => boolean) {
  for (const [id, d] of details) {
    if (inView(d.folderId) && !d.seen) details.set(id, { ...d, seen: true });
  }
  for (const [k, list] of threads) {
    if (list.some((m) => inView(m.folderId) && !m.seen)) {
      threads.set(
        k,
        list.map((m) => (inView(m.folderId) ? { ...m, seen: true } : m)),
      );
    }
  }
  for (const p of readerPool.panes) {
    if (inView(p.detail.folderId) && !p.detail.seen) p.detail = { ...p.detail, seen: true };
    if (p.thread.some((m) => inView(m.folderId) && !m.seen)) {
      p.thread = p.thread.map((m) => (inView(m.folderId) ? { ...m, seen: true } : m));
    }
  }
}

/** Removes deleted/moved messages from the caches and keep-alive pool. */
export function evictMessages(ids: number[]) {
  if (!ids.length) return;
  const set = new Set(ids);
  for (const id of set) {
    if (details.delete(id)) {
      detailBytes = Math.max(0, detailBytes - (detailSizes.get(id) ?? 0));
      detailSizes.delete(id);
    }
    threads.delete(id);
    shownImages.delete(id);
    heights.delete(id);
    evictDocsFor(id);
  }
  // Remove any thread cache entry referencing a removed message so it refreshes cleanly.
  for (const [k, list] of threads) {
    if (list.some((m) => set.has(m.id))) threads.delete(k);
  }
  readerPool.panes = readerPool.panes.filter((p) => !set.has(p.id) && !p.thread.some((m) => set.has(m.id)));
}

/** Clears every cache (e.g. when an account is removed or in tests). */
export function clearMailCache() {
  details.clear();
  detailSizes.clear();
  detailBytes = 0;
  threads.clear();
  shownImages.clear();
  docs.clear();
  heights.clear();
  readerPool.panes = [];
}

/** Test-only / diagnostic snapshot of cache sizes. */
export function cacheStats() {
  return {
    details: details.size,
    detailBytes,
    threads: threads.size,
    panes: readerPool.panes.length,
    docs: docs.size,
    heights: heights.size,
  };
}
