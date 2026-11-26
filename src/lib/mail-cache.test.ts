import { beforeEach, describe, expect, it } from "vitest";
import type { Detail, Summary } from "./api";
import {
  cacheStats,
  cachedDetailsFor,
  cachedFrameDoc,
  cachedPhishingWarnings,
  cachedRemoteImageUrls,
  clearMailCache,
  evictMessages,
  getCachedDetail,
  getCachedFrameHeight,
  getCachedThread,
  hasShownImages,
  markCachedFolderRead,
  markShownImages,
  peekCachedDetail,
  pruneBackgroundCaches,
  putDetail,
  putThread,
  readerPool,
  setCachedFrameHeight,
  updateCachedFlagged,
  updateCachedSeen,
  upsertReaderPane,
} from "./mail-cache.svelte";

function makeDetail(id: number, overrides: Partial<Detail> = {}): Detail {
  return {
    id,
    accountId: 1,
    folderId: 10,
    fromName: "Alice",
    fromAddr: "alice@example.com",
    subject: `Subject ${id}`,
    snippet: `Snippet ${id}`,
    date: 1700000000 + id,
    seen: false,
    flagged: false,
    hasAttachments: false,
    threadId: `t-${id}`,
    count: 1,
    ids: [id],
    snoozedUntil: null,
    to: [{ name: "Bob", email: "bob@example.com" }],
    cc: [],
    bcc: [],
    replyTo: [],
    messageId: `<m-${id}@example.com>`,
    inReplyTo: null,
    references: null,
    html: `<p>Body ${id}</p>`,
    text: `Body ${id}`,
    attachments: [],
    listId: null,
    ...overrides,
  };
}

describe("mail-cache", () => {
  beforeEach(() => {
    clearMailCache();
  });

  it("caches details and threads and serves them synchronously", () => {
    const d1 = makeDetail(1);
    putDetail(d1);
    putThread(1, [d1]);

    expect(getCachedDetail(1)).toEqual(d1);
    expect(peekCachedDetail(1)).toEqual(d1);
    expect(getCachedThread(1)).toEqual([d1]);
    expect(cachedDetailsFor([1, 2])).toEqual({ 1: d1 });
  });

  it("keeps readerPool panes in stable DOM order when switching back and forth", () => {
    const d1 = makeDetail(1);
    const d2 = makeDetail(2);
    putDetail(d1);
    putDetail(d2);

    upsertReaderPane(1, d1, [d1], [1]);
    upsertReaderPane(2, d2, [d2], [2]);
    expect(readerPool.panes.map((p) => p.id)).toEqual([1, 2]);

    // Switching back to 1 must NOT move pane 1 in the array (which would reload its iframe).
    upsertReaderPane(1, d1, [d1], [1]);
    expect(readerPool.panes.map((p) => p.id)).toEqual([1, 2]);
    expect(readerPool.panes[0].lastUsed).toBeGreaterThan(readerPool.panes[1].lastUsed);
  });

  it("evicts the least-recently-used pane when exceeding the keep-alive pool cap", () => {
    for (let i = 1; i <= 4; i++) {
      const d = makeDetail(i);
      putDetail(d);
      upsertReaderPane(i, d, [d], [i]);
    }
    // Touch pane 1 so pane 2 becomes the least recently used.
    const d1 = getCachedDetail(1)!;
    upsertReaderPane(1, d1, [d1], [1]);

    const d5 = makeDetail(5);
    putDetail(d5);
    upsertReaderPane(5, d5, [d5], [5]);

    const ids = readerPool.panes.map((p) => p.id);
    expect(ids).toHaveLength(4);
    expect(ids).toContain(1);
    expect(ids).toContain(5);
    expect(ids).not.toContain(2);
  });

  it("keeps seen, flagged, and folder-read state coherent across caches and panes", () => {
    const d1 = makeDetail(1, { folderId: 10, seen: false, flagged: false });
    const d2 = makeDetail(2, { folderId: 20, seen: false, flagged: false });
    putDetail(d1);
    putDetail(d2);
    putThread(1, [d1]);
    upsertReaderPane(1, d1, [d1], [1]);

    updateCachedSeen([1], true);
    expect(getCachedDetail(1)?.seen).toBe(true);
    expect(getCachedThread(1)?.[0].seen).toBe(true);
    expect(readerPool.panes[0].detail.seen).toBe(true);

    updateCachedFlagged([1], true);
    expect(getCachedDetail(1)?.flagged).toBe(true);
    expect(readerPool.panes[0].detail.flagged).toBe(true);

    markCachedFolderRead((fid) => fid === 20);
    expect(getCachedDetail(2)?.seen).toBe(true);
  });

  it("evicts deleted/moved messages from caches and keep-alive panes", () => {
    const d1 = makeDetail(1);
    const d2 = makeDetail(2);
    putDetail(d1);
    putDetail(d2);
    putThread(1, [d1]);
    upsertReaderPane(1, d1, [d1], [1]);
    upsertReaderPane(2, d2, [d2], [2]);
    markShownImages(1);
    setCachedFrameHeight(1, 420);
    cachedFrameDoc(1, "<p>Body 1</p>", false, false);

    evictMessages([1]);
    expect(getCachedDetail(1)).toBeUndefined();
    expect(getCachedThread(1)).toBeUndefined();
    expect(hasShownImages(1)).toBe(false);
    expect(getCachedFrameHeight(1)).toBeUndefined();
    expect(readerPool.panes.map((p) => p.id)).toEqual([2]);
  });

  it("caches prepared frameDoc and invalidates when detail is replaced", () => {
    const d1 = makeDetail(1, { html: "<p>v1</p>" });
    putDetail(d1);
    const doc1 = cachedFrameDoc(1, "<p>v1</p>", false, false);
    expect(cachedFrameDoc(1, "<p>v1</p>", false, false)).toBe(doc1);
    expect(cacheStats().docs).toBe(1);

    // Updating a draft invalidates its prepared doc.
    putDetail(makeDetail(1, { html: "<p>v2</p>" }));
    expect(cacheStats().docs).toBe(0);
  });

  it("pins details belonging to active keep-alive panes during cache eviction", () => {
    const d1 = makeDetail(1);
    putDetail(d1);
    upsertReaderPane(1, d1, [d1], [1]);

    // Insert 50 more messages (exceeding MAX_DETAILS = 40) without putting them in the pane pool.
    for (let i = 2; i <= 55; i++) {
      putDetail(makeDetail(i));
    }
    expect(getCachedDetail(1)).toBeDefined();
    expect(cacheStats().details).toBeLessThanOrEqual(40);
  });

  it("shares a single DOMParser analysis across frameDoc, phishing warnings, and remote images", () => {
    const html = '<p>Hi <a href="https://evil.test/login">https://bank.example/login</a></p><img src="https://cdn.example/pic.png" width="200" height="100">';
    const warnings = cachedPhishingWarnings(42, html, false);
    expect(warnings.length).toBeGreaterThan(0);
    expect(cacheStats().analyzed).toBe(1);

    const urls = cachedRemoteImageUrls(42, html, false);
    expect(urls).toEqual(["https://cdn.example/pic.png"]);
    expect(cacheStats().analyzed).toBe(1);

    const doc = cachedFrameDoc(42, html, true, false, false);
    expect(doc).toContain("cdn.example/pic.png");
    expect(cacheStats().analyzed).toBe(1);
    expect(cacheStats().docBytes).toBe(doc.length);
  });

  it("prunes hidden panes and enforces doc byte budget", () => {
    for (let i = 1; i <= 3; i++) {
      const d = makeDetail(i);
      putDetail(d);
      upsertReaderPane(i, d, [d], [i]);
    }
    pruneBackgroundCaches(2);
    expect(readerPool.panes.map((p) => p.id)).toEqual([2]);

    // Large prepared docs exceeding MAX_DOC_BYTES (16 MB) evict older entries.
    const big = `<p>${"a".repeat(6 * 1024 * 1024)}</p>`;
    cachedFrameDoc(100, big, false, false);
    cachedFrameDoc(101, big, false, false);
    cachedFrameDoc(102, big, false, false);
    expect(cacheStats().docBytes).toBeLessThanOrEqual(16 * 1024 * 1024);
    expect(cacheStats().docs).toBeLessThan(3);
  });
});
