import { api } from "./api";

export type Action =
  | "next" | "prev" | "reply" | "replyAll" | "forward" | "compose" | "archive" | "delete" | "junk"
  | "toggleUnread" | "toggleFlag" | "search" | "sync" | "settings" | "send";

export const ACTIONS: { id: Action; label: string; group: string }[] = [
  { id: "next", label: "Next message", group: "Navigate" },
  { id: "prev", label: "Previous message", group: "Navigate" },
  { id: "search", label: "Search", group: "Navigate" },
  { id: "compose", label: "New message", group: "Write" },
  { id: "reply", label: "Reply", group: "Write" },
  { id: "replyAll", label: "Reply all", group: "Write" },
  { id: "forward", label: "Forward", group: "Write" },
  { id: "send", label: "Send", group: "Write" },
  { id: "archive", label: "Archive", group: "Organize" },
  { id: "delete", label: "Delete", group: "Organize" },
  { id: "junk", label: "Junk / Not junk", group: "Organize" },
  { id: "toggleUnread", label: "Mark read / unread", group: "Organize" },
  { id: "toggleFlag", label: "Flag", group: "Organize" },
  { id: "sync", label: "Check for mail", group: "App" },
  { id: "settings", label: "Settings", group: "App" },
];

export const DEFAULTS: Record<Action, string[]> = {
  next: ["j", "arrowdown"],
  prev: ["k", "arrowup"],
  search: ["/", "mod+f"],
  compose: ["c", "mod+n"],
  reply: ["r"],
  replyAll: ["a"],
  forward: ["f"],
  send: ["mod+enter"],
  archive: ["e"],
  delete: ["#", "backspace", "delete", "mod+backspace"],
  junk: ["mod+shift+j"],
  toggleUnread: ["u"],
  toggleFlag: ["s"],
  sync: ["mod+r"],
  settings: ["mod+,"],
};

/** Actions that may fire while focus is in a text field (they all need a modifier). */
const WHILE_TYPING = new Set<Action>(["compose", "search", "sync", "settings"]);

export const keymap = $state({
  bindings: structuredClone(DEFAULTS) as Record<Action, string[]>,
  recording: false,
});

/** Normalizes a keydown into "mod+shift+k" form. Null for bare modifier presses. */
export function comboOf(e: KeyboardEvent): string | null {
  if (["Meta", "Control", "Alt", "Shift", "CapsLock", "Fn"].includes(e.key)) return null;
  let key = e.key.toLowerCase();
  // Option changes the character on macOS (⌥A → å); use the physical key instead.
  if (e.altKey && /^Key[A-Z]$/.test(e.code)) key = e.code.slice(3).toLowerCase();
  if (e.altKey && /^Digit\d$/.test(e.code)) key = e.code.slice(5);
  if (key === " ") key = "space";
  const parts: string[] = [];
  if (e.metaKey) parts.push("mod");
  if (e.ctrlKey) parts.push("ctrl");
  if (e.altKey) parts.push("alt");
  // Symbols like "#" already include Shift; only record it for letters and named keys.
  if (e.shiftKey && (/^[a-z]$/.test(key) || key.length > 1)) parts.push("shift");
  parts.push(key);
  return parts.join("+");
}

const KEY_NAMES: Record<string, string> = {
  mod: "⌘", ctrl: "⌃", alt: "⌥", shift: "⇧",
  arrowdown: "↓", arrowup: "↑", arrowleft: "←", arrowright: "→",
  enter: "↩", backspace: "⌫", delete: "⌦", escape: "Esc", tab: "⇥", space: "Space",
  pageup: "PgUp", pagedown: "PgDn", home: "Home", end: "End",
};

export function display(combo: string): string {
  return combo
    .split("+")
    .map((p) => KEY_NAMES[p] ?? (p.length === 1 ? p.toUpperCase() : p[0].toUpperCase() + p.slice(1)))
    .join("");
}

/** "Reply  R" style tooltip text. */
export function hint(label: string, action: Action): string {
  const first = keymap.bindings[action][0];
  return first ? `${label}  ${display(first)}` : label;
}

export function match(combo: string): Action | undefined {
  return (Object.keys(keymap.bindings) as Action[]).find((a) => keymap.bindings[a].includes(combo));
}

export function canFireWhileTyping(a: Action): boolean {
  return WHILE_TYPING.has(a);
}

export function usesModifier(combo: string): boolean {
  return /(^|\+)(mod|ctrl|alt)\+/.test(combo);
}

async function save() {
  await api.setSetting("shortcuts", JSON.stringify(keymap.bindings));
}

/** Adds a binding. Returns the action it was taken from, if any. */
export async function bind(action: Action, combo: string): Promise<Action | undefined> {
  const prev = match(combo);
  if (prev === action) return undefined;
  if (prev) keymap.bindings[prev] = keymap.bindings[prev].filter((c) => c !== combo);
  keymap.bindings[action] = [...keymap.bindings[action], combo];
  await save();
  return prev;
}

export async function unbind(action: Action, combo: string) {
  keymap.bindings[action] = keymap.bindings[action].filter((c) => c !== combo);
  await save();
}

export async function resetShortcuts() {
  keymap.bindings = structuredClone(DEFAULTS);
  await save();
}

export async function loadShortcuts() {
  try {
    const saved = JSON.parse((await api.getSetting("shortcuts")) || "{}") as Partial<Record<Action, string[]>>;
    const merged = structuredClone(DEFAULTS);
    for (const a of Object.keys(merged) as Action[]) {
      if (Array.isArray(saved[a])) merged[a] = saved[a]!.filter((c) => typeof c === "string");
    }
    keymap.bindings = merged;
  } catch {
    keymap.bindings = structuredClone(DEFAULTS);
  }
}

export function labelOf(a: Action): string {
  return ACTIONS.find((x) => x.id === a)?.label ?? a;
}
