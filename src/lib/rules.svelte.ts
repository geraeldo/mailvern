// Rules: what the Rules settings show and edit, and the words used to describe a rule.
import { api, type Detail, type Rule, type RuleAction, type RuleCondition, type RuleFacts, type RuleField, type RuleOp, type SnoozeUntil } from "./api";
import { accountById, accountLabel, folderById, openSettings, toast, ui } from "./state.svelte";

export const rulesUi = $state({
  rules: [] as Rule[],
  loaded: false,
  /** The rule being edited (id 0 for a new one), or null for the list. */
  draft: null as Rule | null,
  /** A message to describe a rule from ("Create Rule › Describe…"): the AI starts from its headers. */
  source: null as RuleFacts | null,
});

export async function loadRules() {
  try {
    rulesUi.rules = await api.rules();
  } catch (e) {
    toast(`Couldn't load rules: ${e}`);
  }
  rulesUi.loaded = true;
}

export const FIELDS: { value: RuleField; label: string; flag?: boolean }[] = [
  { value: "from", label: "From" },
  { value: "fromDomain", label: "Sender's domain" },
  { value: "to", label: "To" },
  { value: "cc", label: "Cc" },
  { value: "toOrCc", label: "To or Cc" },
  { value: "subject", label: "Subject" },
  { value: "body", label: "Body" },
  { value: "listId", label: "Mailing list" },
  { value: "onlyToMe", label: "Sent only to me", flag: true },
  { value: "ccMe", label: "I'm in Cc", flag: true },
  { value: "hasAttachment", label: "Has an attachment", flag: true },
];

export const OPS: { value: RuleOp; label: string }[] = [
  { value: "contains", label: "contains" },
  { value: "is", label: "is" },
  { value: "startsWith", label: "starts with" },
  { value: "endsWith", label: "ends with" },
  { value: "regex", label: "matches pattern" },
];

export const SNOOZES: { value: SnoozeUntil; label: string }[] = [
  { value: "laterToday", label: "for 3 hours" },
  { value: "tomorrow", label: "until tomorrow 8:00" },
  { value: "nextWeek", label: "until Monday 8:00" },
];

export const ACTIONS: { value: RuleAction["type"]; label: string; detail?: string }[] = [
  { value: "move", label: "Move to folder" },
  { value: "archive", label: "Archive" },
  { value: "trash", label: "Delete" },
  { value: "junk", label: "Move to Junk" },
  { value: "snooze", label: "Snooze" },
  { value: "markRead", label: "Mark as read" },
  { value: "flag", label: "Flag" },
  { value: "skipNotification", label: "Don't notify" },
  { value: "forward", label: "Forward to", detail: "Sends mail as you" },
  { value: "reply", label: "Auto-reply", detail: "Sends mail as you" },
];

export const isFlag = (f: RuleField) => FIELDS.find((x) => x.value === f)?.flag ?? false;
export const sends = (a: RuleAction) => a.type === "forward" || a.type === "reply";
export const relocates = (a: RuleAction) => ["move", "archive", "trash", "junk", "snooze"].includes(a.type);

export function blankRule(): Rule {
  return {
    id: 0,
    name: "",
    enabled: true,
    accountId: null,
    matchAll: true,
    conditions: [],
    exceptions: [],
    actions: [],
    stop: false,
    origin: "user",
    needsApproval: false,
    pausedReason: null,
  };
}

export function blankAction(type: RuleAction["type"], accountId: number | null): RuleAction {
  switch (type) {
    case "move": {
      const f = ui.folders.find((f) => f.accountId === accountId && !f.role) ?? ui.folders.find((f) => f.accountId === accountId);
      return { type, folderId: f?.id ?? 0 };
    }
    case "snooze": return { type, until: "tomorrow" };
    case "forward": return { type, to: [] };
    case "reply": return { type, subject: null, text: "" };
    default: return { type };
  }
}

function condText(c: RuleCondition): string {
  const field = FIELDS.find((f) => f.value === c.field)?.label ?? c.field;
  if (isFlag(c.field)) return field.toLowerCase();
  const op = OPS.find((o) => o.value === c.op)?.label ?? c.op;
  return `${field.toLowerCase()} ${op} “${c.value}”`;
}

export function actionText(a: RuleAction): string {
  switch (a.type) {
    case "move": return `move to ${folderById(a.folderId)?.name ?? "a folder"}`;
    case "snooze": return `snooze ${SNOOZES.find((s) => s.value === a.until)?.label ?? ""}`;
    case "forward": return `forward to ${a.to.join(", ") || "…"}`;
    case "reply": return "auto-reply";
    default: return (ACTIONS.find((x) => x.value === a.type)?.label ?? a.type).toLowerCase();
  }
}

/** One line for the list: "If from contains “github”: archive, mark as read". */
export function ruleSummary(r: Rule): string {
  const when = r.conditions.length ? r.conditions.map(condText).join(r.matchAll ? " and " : " or ") : "any new mail";
  const except = r.exceptions.length ? `, except ${r.exceptions.map(condText).join(" or ")}` : "";
  const where = r.accountId != null ? ` in ${(accountById(r.accountId) && accountLabel(accountById(r.accountId)!)) ?? "an account"}` : "";
  const what = r.actions.map(actionText).join(", ");
  return `If ${when}${except}${where}: ${what}`;
}

/** Opens Settings › Rules on the editor, on `draft` or a blank rule. */
export function openRuleEditor(draft: Rule | null = null) {
  rulesUi.draft = structuredClone($state.snapshot(draft ?? blankRule())) as Rule;
  rulesUi.source = null;
  openSettings("rules");
}

/** Opens Settings › Rules on the describe box, with this message as the example to start from. */
export async function describeFromMessage(d: Detail) {
  try {
    const [facts] = await api.ruleFacts([d.id]);
    rulesUi.draft = null;
    rulesUi.source = facts ?? null;
    openSettings("rules");
  } catch (e) {
    toast(String(e));
  }
}

export type FromMessage = "sender" | "domain" | "list" | "subject";

/** A new rule that catches mail like this message. */
export function ruleFromMessage(d: Detail, kind: FromMessage): Rule {
  const r = blankRule();
  r.accountId = d.accountId;
  const domain = d.fromAddr.split("@")[1] ?? "";
  const name = d.fromName || d.fromAddr;
  switch (kind) {
    case "sender":
      r.name = `From ${name}`;
      r.conditions = [{ field: "from", op: "is", value: d.fromAddr }];
      break;
    case "domain":
      r.name = `From ${domain}`;
      r.conditions = [{ field: "fromDomain", op: "is", value: domain }];
      break;
    case "list": {
      const id = d.listId?.match(/<([^>]+)>/)?.[1] ?? d.listId ?? "";
      r.name = `List ${id}`;
      r.conditions = [{ field: "listId", op: "contains", value: id }];
      break;
    }
    case "subject":
      r.name = `“${d.subject}”`;
      r.conditions = [{ field: "subject", op: "contains", value: d.subject.replace(/^\s*((re|fwd?|aw|wg)\s*:\s*)+/i, "") }];
      break;
  }
  r.actions = [{ type: "archive" }];
  return r;
}
