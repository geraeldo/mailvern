// The slice of pi-ai the app uses. Only the OpenAI-compatible provider is imported (OpenRouter
// speaks it); pi-ai's main entry would bundle every provider and their Node-only dependencies.
import type { AssistantMessageEventStream, Context, Model, Tool, ToolCall } from "@mariozechner/pi-ai";
import { Type, type TSchema } from "typebox";
import { Value } from "typebox/value";

export type { AssistantMessage, Context, ImageContent, TextContent, Tool, ToolCall, ToolResultMessage } from "@mariozechner/pi-ai";
export { Type };

export type StreamOptions = { apiKey: string; signal?: AbortSignal; maxTokens?: number };

// Loaded on first use: it brings pi-ai's model catalog, which the app doesn't need at launch.
let provider: Promise<typeof import("@mariozechner/pi-ai/openai-completions")> | null = null;

export async function stream(model: Model<"openai-completions">, context: Context, options: StreamOptions): Promise<AssistantMessageEventStream> {
  provider ??= import("@mariozechner/pi-ai/openai-completions");
  provider.catch(() => (provider = null));
  return (await provider).streamOpenAICompletions(model, context, options);
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
