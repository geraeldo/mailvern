// The slice of Pi (pi-ai) the app uses. Each wire protocol adapter is lazy-loaded on first use
// so the provider SDKs aren't pulled into the initial bundle at launch.
import {
  normalizeContext,
  type AssistantMessageEventStream,
  type Context,
  type Model,
  type Tool,
  type ToolCall,
  type TranscriptContext,
} from "@earendil-works/pi-ai";
import { Type, type TSchema } from "typebox";
import { Value } from "typebox/value";
import { aiFetch } from "./fetch";
import type { SupportedApi } from "./providers";

export type { AssistantMessage, Context, ImageContent, Message, TextContent, Tool, ToolCall, ToolResultMessage } from "@earendil-works/pi-ai";
export type { SupportedApi };
export { Type };

export type StreamOptions = { apiKey: string; signal?: AbortSignal; maxTokens?: number };

type StreamMod = {
  stream: (model: any, context: TranscriptContext, options?: any) => AssistantMessageEventStream;
};

const apiCache = new Map<SupportedApi, Promise<StreamMod>>();

function loadApi(api: SupportedApi): Promise<StreamMod> {
  let p = apiCache.get(api);
  if (p) return p;
  switch (api) {
    case "openai-completions":
      p = import("@earendil-works/pi-ai/api/openai-completions");
      break;
    case "openai-responses":
      p = import("@earendil-works/pi-ai/api/openai-responses");
      break;
    case "anthropic-messages":
      p = import("@earendil-works/pi-ai/api/anthropic-messages");
      break;
    case "google-generative-ai":
      p = import("@earendil-works/pi-ai/api/google-generative-ai");
      break;
    case "mistral-conversations":
      p = import("@earendil-works/pi-ai/api/mistral-conversations");
      break;
  }
  p.catch(() => apiCache.delete(api));
  apiCache.set(api, p);
  return p;
}

/**
 * Streams one reply. Takes a context with a system prompt and tools, or a transcript whose
 * system messages already carry them (what the agent loop passes).
 */
export async function stream(
  model: Model<SupportedApi>,
  context: Context | TranscriptContext,
  options: StreamOptions,
): Promise<AssistantMessageEventStream> {
  const mod = await loadApi(model.api);
  return mod.stream(model, normalizeContext(context as Context), { ...options, fetch: aiFetch });
}

/** A string enum as plain JSON Schema (`enum`), which every provider accepts. */
export function StringEnum<T extends string>(values: readonly T[], options: { description?: string } = {}) {
  return Type.Unsafe<T>({ type: "string", enum: [...values], ...(options.description ? { description: options.description } : {}) });
}

/** Checks a tool call's arguments against its schema (coercing "12" to 12 and the like). */
export function validateToolCall(tools: Tool[], call: ToolCall): Record<string, any> {
  const tool = tools.find((t) => t.name === call.name);
  if (!tool) throw new Error(`There is no tool called "${call.name}".`);
  const schema = tool.parameters as TSchema;
  const args = structuredClone(call.arguments ?? {});
  Value.Convert(schema, args);
  if (Value.Check(schema, args)) return args;
  const errors = [...Value.Errors(schema, args)].map((e) => `- ${e.instancePath.replace(/^\//, "").replace(/\//g, ".") || "arguments"}: ${e.message}`);
  throw new Error(`Invalid arguments for ${call.name}:\n${errors.join("\n")}`);
}
