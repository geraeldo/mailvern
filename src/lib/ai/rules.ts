// Rules from plain language: the model turns "move GitHub PR mail to Dev" into a rule, which
// opens in the rule editor for the user to check and save. Nothing is saved here.
import { stream, Type, StringEnum, validateToolCall, type Context, type Tool, type ToolCall } from "./llm";
import { api, type Rule, type RuleAction, type RuleCondition } from "../api";
import { ui, accountById } from "../state.svelte";
import { ai, apiKey, piModel } from "./config.svelte";
import { FIELDS, OPS, ACTIONS, SNOOZES, blankRule } from "../rules.svelte";

const MAX_STEPS = 3;
const ALL = "all accounts";

/** Folders a rule can move mail to, by the name the model sees. */
function folderChoices(): Map<string, { id: number; accountId: number }> {
  const many = ui.accounts.length > 1;
  const out = new Map<string, { id: number; accountId: number }>();
  for (const f of ui.folders) {
    if (f.role === "inbox" || f.role === "snoozed") continue;
    const acc = accountById(f.accountId);
    out.set(many && acc ? `${acc.email} › ${f.name}` : f.name, { id: f.id, accountId: f.accountId });
  }
  return out;
}

function tools(folders: string[]): Tool[] {
  const cond = Type.Object({
    field: StringEnum(FIELDS.map((f) => f.value), {
      description:
        "from: sender name or address. fromDomain: the part after @ (also matches subdomains with op is). to, cc, toOrCc: recipients. subject. body: the message text. listId: the List-Id of mailing lists and notifications. onlyToMe, ccMe, hasAttachment take no op or value.",
    }),
    op: Type.Optional(StringEnum(OPS.map((o) => o.value), { description: "How value is compared, case-insensitively. regex: a regular expression, only when words can't say it." })),
    value: Type.Optional(Type.String({ description: "What to look for. Leave out for onlyToMe, ccMe, hasAttachment." })),
  });
  const action = Type.Object({
    type: StringEnum(ACTIONS.map((a) => a.value), {
      description: "move (needs folder), archive, trash (moves to Trash), junk, snooze (needs until), markRead, flag, skipNotification, forward (needs to), reply (an auto-reply; needs text).",
    }),
    folder: Type.Optional(folders.length ? StringEnum(folders, { description: "For move: one of the user's folders, exactly as listed." }) : Type.String()),
    until: Type.Optional(StringEnum(SNOOZES.map((s) => s.value))),
    to: Type.Optional(Type.Array(Type.String(), { description: "For forward: email addresses." })),
    subject: Type.Optional(Type.String({ description: "For reply: a subject; leave out for “Re: <their subject>”." })),
    text: Type.Optional(Type.String({ description: "For reply: the reply text, in the language of the request." })),
  });
  return [
    {
      name: "propose_rule",
      description: "Propose the rule. It opens in the editor; the user checks and saves it.",
      parameters: Type.Object({
        name: Type.String({ description: "A short name, 2–5 words." }),
        account: StringEnum([ALL, ...ui.accounts.map((a) => a.email)], { description: "Which account's new mail it applies to." }),
        matchAll: Type.Boolean({ description: "true: all conditions must hold. false: any one." }),
        conditions: Type.Array(cond, { description: "Empty: every new message." }),
        exceptions: Type.Optional(Type.Array(cond, { description: "The rule doesn't apply when any of these holds." })),
        actions: Type.Array(action, { minItems: 1 }),
        stop: Type.Optional(Type.Boolean({ description: "Don't apply later rules to mail this one matches." })),
      }),
    },
    {
      name: "cannot_do",
      description: "Rules can't do what the user asked. Say why in one sentence, and what a rule could do instead if anything.",
      parameters: Type.Object({ reason: Type.String() }),
    },
  ];
}

function systemPrompt(folders: string[]): string {
  const me = ui.accounts.map((a) => `${a.name ? `${a.name} ` : ""}<${a.email}>`).join(", ");
  return `You turn the user's description into a mail rule for Mailvern, their mail app. Call propose_rule once, or cannot_do.

How rules work: when a new message arrives in an Inbox, Mailvern checks each rule's conditions and does its actions. Rules only act on new Inbox mail, never on sent mail or other folders, and only while Mailvern is open.

The user's accounts: ${me || "none"}. "Me", "I" and "my" refer to them.
Folders a rule can move mail to: ${folders.length ? folders.map((f) => `"${f}"`).join(", ") : "none"}.
Today: ${new Date().toDateString()}.

- Prefer simple word conditions. For a company or service, use fromDomain is "example.com". For newsletters and notifications, a listId or fromDomain condition is most reliable.
- Only move to a folder from the list. If the folder the user names doesn't exist, use cannot_do and say so (the user can create the folder first).
- A move needs one account: pick the account the folder belongs to.
- Write auto-reply text in the language of the request, short and polite, without promising anything the user didn't say.
- Rules can't delete permanently, label, schedule, or act on a timer; use cannot_do for those.`;
}

function toCondition(c: any): RuleCondition {
  return { field: c.field, op: c.op ?? "contains", value: String(c.value ?? "") };
}

/** The rule in the tool call, with folder names turned into ids. */
function toRule(a: any, folders: Map<string, { id: number; accountId: number }>): Rule {
  const r = blankRule();
  r.name = String(a.name ?? "").trim();
  r.accountId = a.account && a.account !== ALL ? (ui.accounts.find((x) => x.email === a.account)?.id ?? null) : null;
  r.matchAll = a.matchAll !== false;
  r.conditions = (a.conditions ?? []).map(toCondition);
  r.exceptions = (a.exceptions ?? []).map(toCondition);
  r.stop = !!a.stop;
  r.actions = (a.actions ?? []).map((x: any): RuleAction => {
    switch (x.type) {
      case "move": {
        const f = folders.get(x.folder);
        if (!f) throw new Error(`There is no folder “${x.folder}”. Use one from the list.`);
        r.accountId = f.accountId;
        return { type: "move", folderId: f.id };
      }
      case "snooze": return { type: "snooze", until: x.until ?? "tomorrow" };
      case "forward": return { type: "forward", to: (x.to ?? []).map(String) };
      case "reply": return { type: "reply", subject: x.subject || null, text: String(x.text ?? "") };
      default: return { type: x.type };
    }
  });
  return r;
}

/** Turns a description into a rule. Throws with a reason when it can't. */
export async function describeRule(description: string, signal?: AbortSignal): Promise<Rule> {
  const model = ai.search;
  if (!model) throw new Error("Choose a search model in Settings › AI.");
  const folders = folderChoices();
  const names = [...folders.keys()];
  const TOOLS = tools(names);
  const ctx: Context = { systemPrompt: systemPrompt(names), messages: [], tools: TOOLS };
  ctx.messages.push({ role: "user", content: description, timestamp: Date.now() });
  const key = await apiKey();
  for (let n = 0; n < MAX_STEPS; n++) {
    const res = await stream(piModel(model), ctx, { apiKey: key, signal, maxTokens: 4000 });
    for await (const _ of res) if (signal?.aborted) break;
    const msg = await res.result();
    if (msg.stopReason === "aborted" || signal?.aborted) throw new Error("Stopped.");
    if (msg.stopReason === "error") throw new Error(msg.errorMessage || "The model returned an error");
    ctx.messages.push(msg);
    const call = msg.content.find((c): c is ToolCall => c.type === "toolCall");
    if (!call) {
      ctx.messages.push({ role: "user", content: "Call propose_rule or cannot_do.", timestamp: Date.now() });
      continue;
    }
    let problem: string;
    try {
      const args = validateToolCall(TOOLS, call);
      if (call.name === "cannot_do") throw Object.assign(new Error(String(args.reason)), { final: true });
      const rule = toRule(args, folders);
      await api.checkRule(rule);
      return rule;
    } catch (e: any) {
      if (e?.final) throw e;
      problem = e instanceof Error ? e.message : String(e);
    }
    // Answer every tool call so the conversation stays valid, then let it try again.
    for (const c of msg.content.filter((c): c is ToolCall => c.type === "toolCall")) {
      ctx.messages.push({
        role: "toolResult",
        toolCallId: c.id,
        toolName: c.name,
        content: [{ type: "text", text: c === call ? `Error: ${problem}. Fix it and call propose_rule again.` : "Ignored: call one tool." }],
        isError: true,
        timestamp: Date.now(),
      });
    }
  }
  throw new Error("Couldn't turn that into a rule. Try saying it differently, or build it with New rule.");
}
