// AI settings: the OpenRouter key (in the Keychain), the models for search and for writing,
// and whether requests may go to providers that keep or train on data.
import type { Model } from "@earendil-works/pi-ai";
import { api } from "../api";

const OPENROUTER = "https://openrouter.ai/api/v1";

/** What the app keeps about a model, from OpenRouter's model list. Prices are USD per million tokens. */
export type ModelInfo = {
  id: string;
  name: string;
  contextLength: number;
  maxOutput: number | null;
  vision: boolean;
  reasoning: boolean;
  tools: boolean;
  input: number;
  output: number;
};

export type ModelRole = "search" | "write";

/** Picked when a key is first added. Any model can be chosen instead. */
const DEFAULTS: Record<ModelRole, string[]> = {
  search: ["anthropic/claude-haiku-5.5", "google/gemini-3.8-flash", "openai/gpt-5.6-luna"],
  write: ["anthropic/claude-sonnet-5.5", "anthropic/claude-opus-5.5", "google/gemini-4-carbon"],
};

const KEYS = { search: "ai_search_model", write: "ai_write_model", private: "ai_private" } as const;

export const ai = $state({
  loaded: false,
  hasKey: false,
  search: null as ModelInfo | null,
  write: null as ModelInfo | null,
  /** Only use providers that don't store or train on requests. */
  privateOnly: true,
});

/** Search can run: a key and a search model that can use tools. */
export const searchReady = () => ai.hasKey && !!ai.search?.tools;
export const writeReady = () => ai.hasKey && !!ai.write;

function parse(json: string | null): ModelInfo | null {
  if (!json) return null;
  try {
    return JSON.parse(json) as ModelInfo;
  } catch {
    return null;
  }
}

export async function loadAi() {
  try {
    const [key, search, write, priv] = await Promise.all([
      api.aiKey(),
      api.getSetting(KEYS.search),
      api.getSetting(KEYS.write),
      api.getSetting(KEYS.private),
    ]);
    ai.hasKey = !!key;
    cachedKey = key;
    ai.search = parse(search);
    ai.write = parse(write);
    ai.privateOnly = priv !== "0";
  } catch {}
  ai.loaded = true;
}

let cachedKey: string | null = null;

/** The key, read from the Keychain once per launch. */
export async function apiKey(): Promise<string> {
  if (!cachedKey) cachedKey = await api.aiKey();
  if (!cachedKey) throw new Error("Add an OpenRouter API key in Settings › Models.");
  return cachedKey;
}

export type KeyCheck = { ok: true; label: string | null; usage: number | null; limit: number | null } | { ok: false; error: string };

/** Asks OpenRouter whether a key works, and how much it has used. */
export async function checkKey(key: string): Promise<KeyCheck> {
  try {
    const r = await fetch(`${OPENROUTER}/key`, { headers: { Authorization: `Bearer ${key.trim()}` } });
    if (r.status === 401 || r.status === 403) return { ok: false, error: "OpenRouter doesn't accept this key." };
    if (!r.ok) return { ok: false, error: `OpenRouter answered ${r.status}.` };
    const d = (await r.json()).data ?? {};
    return { ok: true, label: d.label ?? null, usage: d.usage ?? null, limit: d.limit ?? null };
  } catch (e) {
    return { ok: false, error: `Couldn't reach OpenRouter: ${e}` };
  }
}

/** Saves the key (empty removes it). New keys get default models if none are chosen yet. */
export async function saveKey(key: string) {
  await api.setAiKey(key);
  cachedKey = key.trim() || null;
  ai.hasKey = !!cachedKey;
  if (!ai.hasKey) return;
  const models = await listModels().catch(() => [] as ModelInfo[]);
  for (const role of ["search", "write"] as const) {
    if (ai[role]) continue;
    const pick = DEFAULTS[role].map((id) => models.find((m) => m.id === id)).find(Boolean) ?? models.find((m) => m.tools);
    if (pick) await setModel(role, pick);
  }
}

export async function setModel(role: ModelRole, m: ModelInfo) {
  ai[role] = m;
  await api.setSetting(KEYS[role], JSON.stringify(m));
}

export async function setPrivateOnly(on: boolean) {
  ai.privateOnly = on;
  await api.setSetting(KEYS.private, on ? "1" : "0");
}

let models: Promise<ModelInfo[]> | null = null;

/** OpenRouter's model list (cached for the session). Batch-only variants are left out. */
export function listModels(): Promise<ModelInfo[]> {
  models ??= (async () => {
    const r = await fetch(`${OPENROUTER}/models`);
    if (!r.ok) throw new Error(`OpenRouter answered ${r.status}`);
    const data: any[] = (await r.json()).data ?? [];
    return data
      .filter((m) => !String(m.id).endsWith(":batch") && (m.architecture?.output_modalities ?? ["text"]).includes("text"))
      .map((m) => {
        const params: string[] = m.supported_parameters ?? [];
        return {
          id: m.id,
          name: String(m.name ?? m.id),
          contextLength: m.context_length ?? m.top_provider?.context_length ?? 128000,
          maxOutput: m.top_provider?.max_completion_tokens ?? null,
          vision: (m.architecture?.input_modalities ?? []).includes("image"),
          reasoning: params.includes("reasoning"),
          tools: params.includes("tools"),
          input: Number(m.pricing?.prompt ?? 0) * 1e6,
          output: Number(m.pricing?.completion ?? 0) * 1e6,
        } satisfies ModelInfo;
      })
      .sort((a, b) => a.name.localeCompare(b.name));
  })();
  models.catch(() => (models = null));
  return models;
}

/** The pi-ai model for a chosen OpenRouter model. */
export function piModel(m: ModelInfo): Model<"openai-completions"> {
  return {
    id: m.id,
    name: m.name,
    api: "openai-completions",
    provider: "openrouter",
    baseUrl: OPENROUTER,
    reasoning: m.reasoning,
    // Send no reasoning setting, so each model thinks as much as it normally does. Without
    // this, pi-ai asks for effort "none", which models with mandatory reasoning refuse.
    thinkingLevelMap: { off: null },
    input: m.vision ? ["text", "image"] : ["text"],
    cost: { input: m.input, output: m.output, cacheRead: 0, cacheWrite: 0 },
    contextWindow: m.contextLength,
    maxTokens: Math.min(m.maxOutput ?? 32000, 32000),
    headers: { "X-Title": "Mailvern" },
    compat: { openRouterRouting: ai.privateOnly ? { data_collection: "deny" } : {} },
  };
}

export const price = (m: ModelInfo) => (m.input === 0 && m.output === 0 ? "Free" : `$${fmt(m.input)} in · $${fmt(m.output)} out per 1M tokens`);
const fmt = (n: number) => (n >= 10 ? n.toFixed(0) : n >= 0.01 ? n.toFixed(2) : n.toFixed(3));
