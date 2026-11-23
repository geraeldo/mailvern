// A conversation as plain text, for pasting into an AI chat, a ticket or notes.

import { api, type Addr, type Detail, type Summary } from "./api";
import { bodyText } from "./ai/text";
import { fileSize, formatAddr, longDate } from "./format";

const list = (xs: Addr[]) => xs.map(formatAddr).join(", ");

/** Replies quote what came before, which is already in the text; forwards carry new content. */
function keepQuotes(d: Detail, i: number) {
  return i === 0 || /^\s*(fwd?|fw)\s*:/i.test(d.subject);
}

function message(d: Detail, i: number): string {
  const head = [
    `From: ${formatAddr({ name: d.fromName || null, email: d.fromAddr })}`,
    d.to.length ? `To: ${list(d.to)}` : null,
    d.cc.length ? `Cc: ${list(d.cc)}` : null,
    `Date: ${longDate(d.date)}`,
    d.attachments.length ? `Attachments: ${d.attachments.map((a) => `${a.filename} (${fileSize(a.size)})`).join(", ")}` : null,
  ].filter(Boolean);
  const body = bodyText(d, !keepQuotes(d, i)) || bodyText(d) || "(empty)";
  return `${head.join("\n")}\n\n${body}`;
}

/** The whole conversation, oldest first. Bodies already loaded in `cache` aren't fetched again. */
export async function threadText(subject: string, thread: Summary[], cache: Record<number, Detail> = {}): Promise<string> {
  const details = await Promise.all(thread.map((m) => cache[m.id] ?? api.message(m.id)));
  const n = details.length;
  const top = [`Subject: ${subject || "(no subject)"}`, n > 1 ? `${n} messages, oldest first.` : null].filter(Boolean).join("\n");
  return [top, ...details.map(message)].join("\n\n---\n\n") + "\n";
}
