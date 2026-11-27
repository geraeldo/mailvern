import { beforeEach, describe, expect, it, vi } from "vitest";

const { streamMock, queryMailMock, apiKeyMock } = vi.hoisted(() => ({
  streamMock: vi.fn(),
  queryMailMock: vi.fn(),
  apiKeyMock: vi.fn(),
}));

vi.mock("./llm", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./llm")>();
  return { ...actual, stream: streamMock };
});

vi.mock("./config.svelte", () => ({
  ai: {
    search: { id: "test-model", name: "Test Model", provider: "openai", api: "openai-completions", vision: false },
  },
  apiKey: apiKeyMock,
  piModel: (m: any) => m,
}));

vi.mock("../api", () => ({
  api: {
    queryMail: queryMailMock,
    summaries: vi.fn().mockResolvedValue([]),
    listMessages: vi.fn().mockResolvedValue([]),
    searchMessages: vi.fn().mockResolvedValue([]),
    counts: vi.fn().mockResolvedValue({ flagged: 0, unread: 0 }),
  },
}));

import { runAiSearch, stopAiSearch } from "./search.svelte";
import { ui, setQuery } from "../state.svelte";

const flush = () => new Promise((r) => setTimeout(r, 0));

describe("stopping AI search", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    apiKeyMock.mockResolvedValue("test-key");
    ui.ai = null;
    ui.query = "";
    ui.accounts = [];
    ui.folders = [];
  });

  it("stops immediately even if the LLM stream does not unwind on abort", async () => {
    let passedSignal: AbortSignal | undefined;
    streamMock.mockImplementation((_model: any, _ctx: any, opts: { signal?: AbortSignal }) => {
      passedSignal = opts.signal;
      return Promise.resolve({
        [Symbol.asyncIterator]() {
          return { next: () => new Promise(() => {}) };
        },
        result: () => new Promise(() => {}),
      });
    });

    runAiSearch("find invoices");
    await flush();

    expect(ui.ai).not.toBeNull();
    expect(ui.ai!.turns[0].running).toBe(true);
    expect(passedSignal?.aborted).toBe(false);

    stopAiSearch();
    await flush();

    expect(passedSignal?.aborted).toBe(true);
    expect(ui.ai!.turns[0].running).toBe(false);
    expect(ui.ai!.turns[0].answer).toBe("Stopped.");
  });

  it("stops immediately while a slow tool call is in flight and ignores late results", async () => {
    let resolveQuery!: (v: any) => void;
    queryMailMock.mockImplementation(() => new Promise((r) => (resolveQuery = r)));

    streamMock.mockResolvedValueOnce({
      async *[Symbol.asyncIterator]() {},
      async result() {
        return {
          role: "assistant",
          stopReason: "toolUse",
          content: [{ type: "toolCall", id: "call-1", name: "search_mail", arguments: { text: "invoice", server: true } }],
          usage: { cost: { total: 0 } },
          timestamp: Date.now(),
        };
      },
    });

    runAiSearch("find old invoices");
    await flush();
    await flush();

    expect(queryMailMock).toHaveBeenCalledTimes(1);
    expect(ui.ai!.turns[0].running).toBe(true);
    expect(ui.ai!.turns[0].steps).toHaveLength(1);
    const stepBefore = ui.ai!.turns[0].steps[0].text;

    stopAiSearch();
    await flush();

    expect(ui.ai!.turns[0].running).toBe(false);
    expect(ui.ai!.turns[0].answer).toBe("Stopped.");
    expect(ui.ai!.turns[0].steps[0].done).toBe(true);

    // Late tool resolution after stopping must not mutate steps or sources.
    resolveQuery({
      total: 1,
      rows: [{ id: 42, folderId: 1, accountId: 1, threadId: null, date: 1700000000, fromName: "A", fromAddr: "a@b.com", subject: "Inv", snippet: "", seen: true, flagged: false, hasAttachments: false, snoozedUntil: null }],
    });
    await flush();

    expect(ui.ai!.turns[0].steps[0].text).toBe(stepBefore);
    expect(ui.ai!.turns[0].sources).toEqual([]);
  });

  it("aborts an in-flight AI search when leaving via setQuery", async () => {
    let passedSignal: AbortSignal | undefined;
    streamMock.mockImplementation((_model: any, _ctx: any, opts: { signal?: AbortSignal }) => {
      passedSignal = opts.signal;
      return Promise.resolve({
        [Symbol.asyncIterator]() {
          return { next: () => new Promise(() => {}) };
        },
        result: () => new Promise(() => {}),
      });
    });

    runAiSearch("where is my receipt?");
    await flush();

    expect(passedSignal?.aborted).toBe(false);
    setQuery("receipt");
    await flush();

    expect(passedSignal?.aborted).toBe(true);
    expect(ui.ai).toBeNull();
  });
});
