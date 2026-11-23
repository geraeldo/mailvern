// The slice of Pi (pi-ai) the app uses. Only the OpenAI-compatible API is used: OpenRouter
// speaks it. It is called directly, without a provider collection or the built-in catalogs.
import { normalizeContext, type AssistantMessageEventStream, type Context, type Model, type Tool, type ToolCall, type TranscriptContext } from "@earendil-works/pi-ai";
import { Type, type TSchema } from "typebox";
import { Value } from "typebox/value";

export type { AssistantMessage, Context, ImageContent, Message, TextContent, Tool, ToolCall, ToolResultMessage } from "@earendil-works/pi-ai";
export { Type };

export type StreamOptions = { apiKey: string; signal?: AbortSignal; maxTokens?: number };

// Loaded on first use: it brings the OpenAI SDK, which the app doesn't need at launch.
let api: Promise<typeof import("@earendil-works/pi-ai/api/openai-completions")> | null = null;

/**
 * Streams one reply. Takes a context with a system prompt and tools, or a transcript whose
 * system messages already carry them (what the agent loop passes).
 */
export async function stream(model: Model<"openai-completions">, context: Context | TranscriptContext, options: StreamOptions): Promise<AssistantMessageEventStream> {
  api ??= import("@earendil-works/pi-ai/api/openai-completions");
  api.catch(() => (api = null));
  return (await api).stream(model, normalizeContext(context as Context), options);
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
