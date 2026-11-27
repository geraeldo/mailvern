// Registry of all single-API-key providers supported by `@earendil-works/pi-ai`.
// Each provider's model catalog is lazy-loaded so startup pays zero bundle cost.
import type { Model } from "@earendil-works/pi-ai";

export type SupportedApi =
  | "openai-completions"
  | "openai-responses"
  | "anthropic-messages"
  | "google-generative-ai"
  | "mistral-conversations";

export const SUPPORTED_APIS: ReadonlySet<string> = new Set<SupportedApi>([
  "openai-completions",
  "openai-responses",
  "anthropic-messages",
  "google-generative-ai",
  "mistral-conversations",
]);

export type ProviderDef = {
  id: string;
  name: string;
  popular?: boolean;
  keyUrl?: string;
  keyUrlLabel?: string;
  keyPlaceholder?: string;
  defaults?: { search: string[]; write: string[] };
  loadCatalog: () => Promise<Model<SupportedApi>[]>;
};

const chatModels = (catalog: Record<string, any>): Model<SupportedApi>[] =>
  Object.values(catalog).filter((m): m is Model<SupportedApi> => m && m.type === "chat" && SUPPORTED_APIS.has(m.api));

export const PROVIDERS: readonly ProviderDef[] = [
  // ---- Popular providers (shown first) ----
  {
    id: "openrouter",
    name: "OpenRouter",
    popular: true,
    keyUrl: "https://openrouter.ai/keys",
    keyUrlLabel: "openrouter.ai/keys",
    keyPlaceholder: "sk-or-…",
    defaults: {
      search: ["anthropic/claude-haiku-5.5", "google/gemini-3.8-flash", "openai/gpt-5.6-luna"],
      write: ["anthropic/claude-sonnet-5.5", "anthropic/claude-opus-5.5", "google/gemini-4-carbon"],
    },
    loadCatalog: () => import("@earendil-works/pi-ai/providers/openrouter.models").then((m) => chatModels(m.OPENROUTER_MODELS)),
  },
  {
    id: "anthropic",
    name: "Anthropic",
    popular: true,
    keyUrl: "https://console.anthropic.com/settings/keys",
    keyUrlLabel: "console.anthropic.com/settings/keys",
    keyPlaceholder: "sk-ant-…",
    defaults: {
      search: ["claude-haiku-5-5", "claude-haiku-4-5", "claude-sonnet-5-5"],
      write: ["claude-sonnet-5-5", "claude-opus-5-5", "claude-sonnet-4-6"],
    },
    loadCatalog: () => import("@earendil-works/pi-ai/providers/anthropic.models").then((m) => chatModels(m.ANTHROPIC_MODELS)),
  },
  {
    id: "openai",
    name: "OpenAI",
    popular: true,
    keyUrl: "https://platform.openai.com/api-keys",
    keyUrlLabel: "platform.openai.com/api-keys",
    keyPlaceholder: "sk-…",
    defaults: {
      search: ["gpt-5.4-mini", "gpt-5-mini", "gpt-4.1-mini"],
      write: ["gpt-5.4", "gpt-5", "gpt-4.1"],
    },
    loadCatalog: () => import("@earendil-works/pi-ai/providers/openai.models").then((m) => chatModels(m.OPENAI_MODELS)),
  },
  {
    id: "google",
    name: "Google Gemini",
    popular: true,
    keyUrl: "https://aistudio.google.com/apikey",
    keyUrlLabel: "aistudio.google.com/apikey",
    keyPlaceholder: "AIza…",
    defaults: {
      search: ["gemini-3.8-flash", "gemini-2.5-flash", "gemini-flash-latest"],
      write: ["gemini-4-carbon", "gemini-2.5-pro", "gemini-pro-latest"],
    },
    loadCatalog: () => import("@earendil-works/pi-ai/providers/google.models").then((m) => chatModels(m.GOOGLE_MODELS)),
  },
  {
    id: "xai",
    name: "xAI",
    popular: true,
    keyUrl: "https://console.x.ai",
    keyUrlLabel: "console.x.ai",
    keyPlaceholder: "xai-…",
    loadCatalog: () => import("@earendil-works/pi-ai/providers/xai.models").then((m) => chatModels(m.XAI_MODELS)),
  },
  {
    id: "mistral",
    name: "Mistral",
    popular: true,
    keyUrl: "https://console.mistral.ai/api-keys",
    keyUrlLabel: "console.mistral.ai/api-keys",
    keyPlaceholder: "API key",
    loadCatalog: () => import("@earendil-works/pi-ai/providers/mistral.models").then((m) => chatModels(m.MISTRAL_MODELS)),
  },
  {
    id: "deepseek",
    name: "DeepSeek",
    popular: true,
    keyUrl: "https://platform.deepseek.com/api_keys",
    keyUrlLabel: "platform.deepseek.com/api_keys",
    keyPlaceholder: "sk-…",
    loadCatalog: () => import("@earendil-works/pi-ai/providers/deepseek.models").then((m) => chatModels(m.DEEPSEEK_MODELS)),
  },
  {
    id: "groq",
    name: "Groq",
    popular: true,
    keyUrl: "https://console.groq.com/keys",
    keyUrlLabel: "console.groq.com/keys",
    keyPlaceholder: "gsk_…",
    loadCatalog: () => import("@earendil-works/pi-ai/providers/groq.models").then((m) => chatModels(m.GROQ_MODELS)),
  },

  // ---- Other pi-ai API-key providers (alphabetical) ----
  {
    id: "ant-ling",
    name: "Ant Ling",
    loadCatalog: () => import("@earendil-works/pi-ai/providers/ant-ling.models").then((m) => chatModels(m.ANT_LING_MODELS)),
  },
  {
    id: "baseten",
    name: "Baseten",
    keyUrl: "https://app.baseten.co/settings/api_keys",
    keyUrlLabel: "app.baseten.co",
    loadCatalog: () => import("@earendil-works/pi-ai/providers/baseten.models").then((m) => chatModels(m.BASETEN_MODELS)),
  },
  {
    id: "cerebras",
    name: "Cerebras",
    keyUrl: "https://cloud.cerebras.ai",
    keyUrlLabel: "cloud.cerebras.ai",
    loadCatalog: () => import("@earendil-works/pi-ai/providers/cerebras.models").then((m) => chatModels(m.CEREBRAS_MODELS)),
  },
  {
    id: "fireworks",
    name: "Fireworks",
    keyUrl: "https://fireworks.ai/account/api-keys",
    keyUrlLabel: "fireworks.ai/account/api-keys",
    loadCatalog: () => import("@earendil-works/pi-ai/providers/fireworks.models").then((m) => chatModels(m.FIREWORKS_MODELS)),
  },
  {
    id: "huggingface",
    name: "Hugging Face",
    keyUrl: "https://huggingface.co/settings/tokens",
    keyUrlLabel: "huggingface.co/settings/tokens",
    keyPlaceholder: "hf_…",
    loadCatalog: () => import("@earendil-works/pi-ai/providers/huggingface.models").then((m) => chatModels(m.HUGGINGFACE_MODELS)),
  },
  {
    id: "kimi-coding",
    name: "Kimi For Coding",
    loadCatalog: () => import("@earendil-works/pi-ai/providers/kimi-coding.models").then((m) => chatModels(m.KIMI_CODING_MODELS)),
  },
  {
    id: "meta",
    name: "Meta",
    loadCatalog: () => import("@earendil-works/pi-ai/providers/meta.models").then((m) => chatModels(m.META_MODELS)),
  },
  {
    id: "minimax",
    name: "MiniMax",
    loadCatalog: () => import("@earendil-works/pi-ai/providers/minimax.models").then((m) => chatModels(m.MINIMAX_MODELS)),
  },
  {
    id: "minimax-cn",
    name: "MiniMax CN",
    loadCatalog: () => import("@earendil-works/pi-ai/providers/minimax-cn.models").then((m) => chatModels(m.MINIMAX_CN_MODELS)),
  },
  {
    id: "moonshotai",
    name: "Moonshot AI",
    keyUrl: "https://platform.moonshot.ai/console/api-keys",
    keyUrlLabel: "platform.moonshot.ai",
    loadCatalog: () => import("@earendil-works/pi-ai/providers/moonshotai.models").then((m) => chatModels(m.MOONSHOTAI_MODELS)),
  },
  {
    id: "moonshotai-cn",
    name: "Moonshot AI CN",
    loadCatalog: () => import("@earendil-works/pi-ai/providers/moonshotai-cn.models").then((m) => chatModels(m.MOONSHOTAI_CN_MODELS)),
  },
  {
    id: "nvidia",
    name: "NVIDIA",
    keyUrl: "https://build.nvidia.com",
    keyUrlLabel: "build.nvidia.com",
    keyPlaceholder: "nvapi-…",
    loadCatalog: () => import("@earendil-works/pi-ai/providers/nvidia.models").then((m) => chatModels(m.NVIDIA_MODELS)),
  },
  {
    id: "opencode",
    name: "OpenCode Zen",
    keyUrl: "https://opencode.ai",
    keyUrlLabel: "opencode.ai",
    loadCatalog: () => import("@earendil-works/pi-ai/providers/opencode.models").then((m) => chatModels(m.OPENCODE_MODELS)),
  },
  {
    id: "opencode-go",
    name: "OpenCode Go",
    keyUrl: "https://opencode.ai",
    keyUrlLabel: "opencode.ai",
    loadCatalog: () => import("@earendil-works/pi-ai/providers/opencode-go.models").then((m) => chatModels(m.OPENCODE_GO_MODELS)),
  },
  {
    id: "qwen-token-plan",
    name: "Qwen Token Plan",
    loadCatalog: () => import("@earendil-works/pi-ai/providers/qwen-token-plan.models").then((m) => chatModels(m.QWEN_TOKEN_PLAN_MODELS)),
  },
  {
    id: "qwen-token-plan-cn",
    name: "Qwen Token Plan CN",
    loadCatalog: () => import("@earendil-works/pi-ai/providers/qwen-token-plan-cn.models").then((m) => chatModels(m.QWEN_TOKEN_PLAN_CN_MODELS)),
  },
  {
    id: "qwen-token-plan-individual",
    name: "Qwen Token Plan Individual",
    loadCatalog: () =>
      import("@earendil-works/pi-ai/providers/qwen-token-plan-individual.models").then((m) => chatModels(m.QWEN_TOKEN_PLAN_INDIVIDUAL_MODELS)),
  },
  {
    id: "qwen-token-plan-individual-cn",
    name: "Qwen Token Plan Individual CN",
    loadCatalog: () =>
      import("@earendil-works/pi-ai/providers/qwen-token-plan-individual-cn.models").then((m) =>
        chatModels(m.QWEN_TOKEN_PLAN_INDIVIDUAL_CN_MODELS),
      ),
  },
  {
    id: "together",
    name: "Together",
    keyUrl: "https://api.together.xyz/settings/api-keys",
    keyUrlLabel: "api.together.xyz",
    loadCatalog: () => import("@earendil-works/pi-ai/providers/together.models").then((m) => chatModels(m.TOGETHER_MODELS)),
  },
  {
    id: "vercel-ai-gateway",
    name: "Vercel AI Gateway",
    keyUrl: "https://vercel.com/ai-gateway",
    keyUrlLabel: "vercel.com/ai-gateway",
    loadCatalog: () => import("@earendil-works/pi-ai/providers/vercel-ai-gateway.models").then((m) => chatModels(m.VERCEL_AI_GATEWAY_MODELS)),
  },
  {
    id: "xiaomi",
    name: "Xiaomi",
    loadCatalog: () => import("@earendil-works/pi-ai/providers/xiaomi.models").then((m) => chatModels(m.XIAOMI_MODELS)),
  },
  {
    id: "xiaomi-token-plan-ams",
    name: "Xiaomi Token Plan AMS",
    loadCatalog: () => import("@earendil-works/pi-ai/providers/xiaomi-token-plan-ams.models").then((m) => chatModels(m.XIAOMI_TOKEN_PLAN_AMS_MODELS)),
  },
  {
    id: "xiaomi-token-plan-cn",
    name: "Xiaomi Token Plan CN",
    loadCatalog: () => import("@earendil-works/pi-ai/providers/xiaomi-token-plan-cn.models").then((m) => chatModels(m.XIAOMI_TOKEN_PLAN_CN_MODELS)),
  },
  {
    id: "xiaomi-token-plan-sgp",
    name: "Xiaomi Token Plan SGP",
    loadCatalog: () => import("@earendil-works/pi-ai/providers/xiaomi-token-plan-sgp.models").then((m) => chatModels(m.XIAOMI_TOKEN_PLAN_SGP_MODELS)),
  },
  {
    id: "zai",
    name: "Z.AI",
    loadCatalog: () => import("@earendil-works/pi-ai/providers/zai.models").then((m) => chatModels(m.ZAI_MODELS)),
  },
  {
    id: "zai-coding-cn",
    name: "Z.AI Coding CN",
    loadCatalog: () => import("@earendil-works/pi-ai/providers/zai-coding-cn.models").then((m) => chatModels(m.ZAI_CODING_CN_MODELS)),
  },
];

export type ProviderId = (typeof PROVIDERS)[number]["id"];

const byId = new Map<string, ProviderDef>(PROVIDERS.map((p) => [p.id, p]));

export function getProvider(id: string | null | undefined): ProviderDef {
  return (id && byId.get(id)) || PROVIDERS[0];
}
