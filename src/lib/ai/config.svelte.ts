// AI settings: active provider, its API key (in the Keychain), the models for search and for
// writing, and OpenRouter's data-collection privacy preference.
import type { Model } from "@earendil-works/pi-ai";
import { api } from "../api";
import { aiFetch } from "./fetch";
import { stream, type SupportedApi } from "./llm";
import { getProvider, PROVIDERS, SUPPORTED_APIS, type ProviderId } from "./providers";

const OPENROUTER = "https://openrouter.ai/api/v1";

/** What the app keeps about a chosen model. Prices are USD per million tokens. */
export type ModelInfo = {
  id: string;
  name: string;
  provider: ProviderId;
  api: SupportedApi;
  baseUrl: string;
  contextLength: number;
  maxOutput: number | null;
  vision: boolean;
  reasoning: boolean;
  tools: boolean;
  input: number;
  output: number;
  cacheRead?: number;
  cacheWrite?: number;
  headers?: Record<string, string>;
  compat?: Record<string, any>;
  thinkingLevelMap?: Record<string, any>;
};

export type ModelRole = "search" | "write";

const KEYS = {
  provider: "ai_provider",
  search: "ai_search_model",
  write: "ai_write_model",
  private: "ai_private",
  configured: "ai_configured",
} as const;

export const ai = $state({
  loaded: false,
  provider: "openrouter" as ProviderId,
  hasKey: false,
  search: null as ModelInfo | null,
  write: null as ModelInfo | null,
  /** OpenRouter only: only use providers that don't store or train on requests. */
  privateOnly: true,
});

/** Search can run: a key and a search model that can use tools. */
export const searchReady = () => ai.hasKey && !!ai.search?.tools;
export const writeReady = () => ai.hasKey && !!ai.write;

function parse(json: string | null): ModelInfo | null {
  if (!json) return null;
  try {
    const raw = JSON.parse(json) as Partial<ModelInfo>;
    if (!raw || typeof raw.id !== "string") return null;
    // Backwards compatibility with models saved before multi-provider support (all were OpenRouter).
    const provider = getProvider(raw.provider ?? "openrouter").id;
    const apiType: SupportedApi = raw.api && SUPPORTED_APIS.has(raw.api) ? raw.api : "openai-completions";
    const baseUrl = raw.baseUrl || OPENROUTER;
    return {
      id: raw.id,
      name: raw.name ?? raw.id,
      provider,
      api: apiType,
      baseUrl,
      contextLength: raw.contextLength ?? 128000,
      maxOutput: raw.maxOutput ?? null,
      vision: !!raw.vision,
      reasoning: !!raw.reasoning,
      tools: raw.tools ?? true,
      input: raw.input ?? 0,
      output: raw.output ?? 0,
      cacheRead: raw.cacheRead,
      cacheWrite: raw.cacheWrite,
      headers: raw.headers,
      compat: raw.compat,
      thinkingLevelMap: raw.thinkingLevelMap,
    };
  } catch {
    return null;
  }
}

export function hydrateAi(settings: Record<string, string>) {
  ai.provider = getProvider(settings[KEYS.provider]).id;
  ai.search = parse(settings[KEYS.search] ?? null);
  ai.write = parse(settings[KEYS.write] ?? null);
  ai.privateOnly = (settings[KEYS.private] ?? null) !== "0";
  const configured = settings[KEYS.configured];
  if (configured === "1") {
    ai.hasKey = true;
    ai.loaded = true;
  } else if (configured === "0") {
    ai.hasKey = false;
    ai.loaded = true;
  } else {
    // One-time migration for upgrades where a key may already exist in Keychain before `ai_configured` was recorded.
    ai.loaded = true;
    api
      .aiKey(ai.provider)
      .then((key) => {
        cachedKey = key;
        cachedProvider = ai.provider;
        ai.hasKey = !!key;
      })
      .catch(() => {});
  }
}

export async function loadAi() {
  try {
    const settings = await api.getSettings([KEYS.provider, KEYS.search, KEYS.write, KEYS.private, KEYS.configured]);
    hydrateAi(settings);
  } catch {
    ai.loaded = true;
  }
}

let cachedKey: string | null = null;
let cachedProvider: ProviderId | null = null;

/** The active provider's key, read from the Keychain once per launch / provider switch. */
export async function apiKey(): Promise<string> {
  if (!cachedKey || cachedProvider !== ai.provider) {
    cachedKey = await api.aiKey(ai.provider);
    cachedProvider = ai.provider;
  }
  if (!cachedKey) {
    const name = getProvider(ai.provider).name;
    throw new Error(`Add a ${name} API key in Settings › Models.`);
  }
  return cachedKey;
}

export type KeyCheck = { ok: true; label: string | null; usage: number | null; limit: number | null } | { ok: false; error: string };

/** Checks whether an API key is accepted by the given provider. */
export async function checkKey(providerId: ProviderId, key: string): Promise<KeyCheck> {
  const trimmed = key.trim();
  const provider = getProvider(providerId);

  if (provider.id === "openrouter") {
    try {
      const r = await aiFetch(`${OPENROUTER}/key`, { headers: { Authorization: `Bearer ${trimmed}` } });
      if (r.status === 401 || r.status === 403) return { ok: false, error: "OpenRouter doesn't accept this key." };
      if (!r.ok) return { ok: false, error: `OpenRouter answered ${r.status}.` };
      const d = (await r.json()).data ?? {};
      return { ok: true, label: d.label ?? null, usage: d.usage ?? null, limit: d.limit ?? null };
    } catch (e) {
      return { ok: false, error: `Couldn't reach OpenRouter: ${e}` };
    }
  }

  try {
    const models = await listModels(provider.id);
    const sample = models[0];

    // Lightweight endpoint checks where the provider exposes a standard models endpoint.
    if (provider.id === "anthropic") {
      const r = await aiFetch("https://api.anthropic.com/v1/models?limit=1", {
        headers: { "x-api-key": trimmed, "anthropic-version": "2023-06-01" },
      });
      if (r.status === 401 || r.status === 403) return { ok: false, error: "Anthropic doesn't accept this key." };
      if (!r.ok) return { ok: false, error: `Anthropic answered ${r.status}.` };
      return { ok: true, label: null, usage: null, limit: null };
    }

    if (provider.id === "google") {
      const r = await aiFetch("https://generativelanguage.googleapis.com/v1beta/models?pageSize=1", {
        headers: { "x-goog-api-key": trimmed },
      });
      if (r.status === 400 || r.status === 401 || r.status === 403) return { ok: false, error: "Google Gemini doesn't accept this key." };
      if (!r.ok) return { ok: false, error: `Google Gemini answered ${r.status}.` };
      return { ok: true, label: null, usage: null, limit: null };
    }

    if (provider.id === "mistral") {
      const r = await aiFetch("https://api.mistral.ai/v1/models", {
        headers: { Authorization: `Bearer ${trimmed}` },
      });
      if (r.status === 401 || r.status === 403) return { ok: false, error: "Mistral doesn't accept this key." };
      if (!r.ok) return { ok: false, error: `Mistral answered ${r.status}.` };
      return { ok: true, label: null, usage: null, limit: null };
    }

    // Try GET <baseUrl>/models for OpenAI-compatible / Bearer-auth providers first.
    if (sample && (sample.api === "openai-completions" || sample.api === "openai-responses")) {
      const base = sample.baseUrl.replace(/\/+$/, "");
      const r = await aiFetch(`${base}/models`, {
        headers: { Authorization: `Bearer ${trimmed}`, ...(sample.headers ?? {}) },
      });
      if (r.status === 401 || r.status === 403) return { ok: false, error: `${provider.name} doesn't accept this key.` };
      if (r.ok) return { ok: true, label: null, usage: null, limit: null };
      // If /models isn't supported (e.g. 404/405), fall through to a tiny stream probe below.
    }

    if (!sample) return { ok: true, label: null, usage: null, limit: null };

    // Universal fallback: minimal 1-token request through pi-ai.
    const ac = new AbortController();
    const timer = setTimeout(() => ac.abort(), 15000);
    try {
      const res = await stream(
        piModel(sample),
        { messages: [{ role: "user", content: "Hi", timestamp: Date.now() }] },
        { apiKey: trimmed, maxTokens: 1, signal: ac.signal },
      );
      for await (const ev of res) {
        if (ev.type === "error") {
          return { ok: false, error: ev.error.errorMessage || `${provider.name} rejected this key.` };
        }
        if (ev.type === "start" || ev.type === "text_delta" || ev.type === "done") {
          ac.abort();
          return { ok: true, label: null, usage: null, limit: null };
        }
      }
      return { ok: true, label: null, usage: null, limit: null };
    } finally {
      clearTimeout(timer);
    }
  } catch (e) {
    return { ok: false, error: `Couldn't reach ${provider.name}: ${e}` };
  }
}

async function pickDefaults(providerId: ProviderId) {
  const provider = getProvider(providerId);
  const models = await listModels(provider.id).catch(() => [] as ModelInfo[]);
  for (const role of ["search", "write"] as const) {
    if (ai[role] && ai[role].provider === provider.id) continue;
    const prefs = provider.defaults?.[role] ?? [];
    const pick =
      prefs.map((id) => models.find((m) => m.id === id)).find(Boolean) ??
      models.find((m) => (role === "search" ? m.tools : true));
    if (pick) await setModel(role, pick);
  }
}

/** Switches the active AI provider and checks if a key is already stored for it. */
export async function setProvider(providerId: ProviderId) {
  const next = getProvider(providerId).id;
  if (ai.provider === next) return;
  ai.provider = next;
  cachedKey = null;
  cachedProvider = null;
  await api.setSetting(KEYS.provider, next);

  // Clear models that belong to a different provider.
  if (ai.search && ai.search.provider !== next) {
    ai.search = null;
    await api.setSetting(KEYS.search, "");
  }
  if (ai.write && ai.write.provider !== next) {
    ai.write = null;
    await api.setSetting(KEYS.write, "");
  }

  // Check if Keychain already has a key for this provider (also updates `ai_configured`).
  try {
    const existing = await api.aiKey(next);
    cachedKey = existing;
    cachedProvider = next;
    ai.hasKey = !!existing;
    if (ai.hasKey) await pickDefaults(next);
  } catch {
    ai.hasKey = false;
  }
}

/** Saves the key for the active provider (empty removes it). New keys get default models if none are chosen yet. */
export async function saveKey(key: string) {
  const providerId = ai.provider;
  await api.setAiKey(key, providerId);
  cachedKey = key.trim() || null;
  cachedProvider = providerId;
  ai.hasKey = !!cachedKey;
  if (!ai.hasKey) return;
  await pickDefaults(providerId);
}

export async function setModel(role: ModelRole, m: ModelInfo) {
  ai[role] = m;
  await api.setSetting(KEYS[role], JSON.stringify(m));
}

export async function setPrivateOnly(on: boolean) {
  ai.privateOnly = on;
  await api.setSetting(KEYS.private, on ? "1" : "0");
}

function fromPiModel(m: Model<SupportedApi>, providerId: ProviderId): ModelInfo {
  return {
    id: m.id,
    name: m.name || m.id,
    provider: providerId,
    api: m.api,
    baseUrl: m.baseUrl,
    contextLength: m.contextWindow ?? 128000,
    maxOutput: m.maxTokens ?? null,
    vision: (m.input ?? []).includes("image"),
    reasoning: !!m.reasoning,
    tools: true,
    input: m.cost?.input ?? 0,
    output: m.cost?.output ?? 0,
    cacheRead: m.cost?.cacheRead,
    cacheWrite: m.cost?.cacheWrite,
    headers: m.headers ? { ...m.headers } : undefined,
    compat: m.compat ? { ...m.compat } : undefined,
    thinkingLevelMap: m.thinkingLevelMap ? { ...m.thinkingLevelMap } : undefined,
  };
}

const modelCache = new Map<ProviderId, Promise<ModelInfo[]>>();

/** Model list for a provider (cached for the session). OpenRouter fetches live; others use pi-ai's catalog. */
export function listModels(providerId: ProviderId = ai.provider): Promise<ModelInfo[]> {
  const id = getProvider(providerId).id;
  let pending = modelCache.get(id);
  if (pending) return pending;

  const provider = getProvider(id);
  pending = (async () => {
    if (id === "openrouter") {
      try {
        const r = await aiFetch(`${OPENROUTER}/models`);
        if (!r.ok) throw new Error(`OpenRouter answered ${r.status}`);
        const data: any[] = (await r.json()).data ?? [];
        return data
          .filter((m) => !String(m.id).endsWith(":batch") && (m.architecture?.output_modalities ?? ["text"]).includes("text"))
          .map((m) => {
            const params: string[] = m.supported_parameters ?? [];
            return {
              id: m.id,
              name: String(m.name ?? m.id),
              provider: "openrouter",
              api: "openai-completions",
              baseUrl: OPENROUTER,
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
      } catch {
        // Fallback to pi-ai's built-in OpenRouter catalog if offline.
      }
    }

    const catalog = await provider.loadCatalog();
    return catalog.map((m) => fromPiModel(m, id)).sort((a, b) => a.name.localeCompare(b.name));
  })();

  pending.catch(() => modelCache.delete(id));
  modelCache.set(id, pending);
  return pending;
}

/** The pi-ai model for any chosen provider model. */
export function piModel(m: ModelInfo): Model<SupportedApi> {
  const isOpenRouter = m.provider === "openrouter";
  const headers = isOpenRouter ? { "X-Title": "Mailvern", ...(m.headers ?? {}) } : m.headers;
  const compat = isOpenRouter
    ? { ...(m.compat ?? {}), openRouterRouting: ai.privateOnly ? { data_collection: "deny" } : {} }
    : m.compat;

  return {
    id: m.id,
    name: m.name,
    api: m.api,
    provider: m.provider,
    baseUrl: m.baseUrl,
    reasoning: m.reasoning,
    // Preserve provider thinkingLevelMap if present; otherwise send no reasoning override so
    // models with mandatory reasoning don't reject effort "none".
    thinkingLevelMap: m.thinkingLevelMap ?? { off: null },
    input: m.vision ? ["text", "image"] : ["text"],
    cost: { input: m.input, output: m.output, cacheRead: m.cacheRead ?? 0, cacheWrite: m.cacheWrite ?? 0 },
    contextWindow: m.contextLength,
    maxTokens: Math.min(m.maxOutput ?? 32000, 32000),
    ...(headers ? { headers } : {}),
    ...(compat ? { compat } : {}),
  } as Model<SupportedApi>;
}

export { getProvider, PROVIDERS, type ProviderId };
export const price = (m: ModelInfo) => (m.input === 0 && m.output === 0 ? "Free" : `$${fmt(m.input)} in · $${fmt(m.output)} out per 1M tokens`);
const fmt = (n: number) => (n >= 10 ? n.toFixed(0) : n >= 0.01 ? n.toFixed(2) : n.toFixed(3));
