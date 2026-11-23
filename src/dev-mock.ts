// Dev-only: sample data so the UI can be previewed in a normal browser (`npm run dev`).
// Never loaded inside the Tauri app.
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";

const accounts = [
  { id: 1, label: "Acme", name: "Alex Rivera", email: "hello@acme.co", imapHost: "", imapPort: 993, smtpHost: "", smtpPort: 465, color: "#2F6BFF", signature: "<b>Alex Rivera</b><br>Acme Co.", position: 0 },
  { id: 2, label: "Personal", name: "Alex Rivera", email: "alex@rivera.me", imapHost: "", imapPort: 993, smtpHost: "", smtpPort: 465, color: "#4FA05A", signature: "— Alex", position: 1 },
  { id: 3, label: "", name: "Acme Billing", email: "billing@acme.co", imapHost: "", imapPort: 993, smtpHost: "", smtpPort: 465, color: "#D9A21B", signature: "", position: 2 },
];

const roles = ["inbox", "drafts", "sent", "archive", "junk", "trash", "snoozed"];
const names: Record<string, string> = { inbox: "Inbox", drafts: "Drafts", sent: "Sent", archive: "Archive", junk: "Spam", trash: "Trash", snoozed: "Snoozed" };
const folders = accounts.flatMap((a) =>
  [...roles.map((r, i) => ({ id: a.id * 10 + i, accountId: a.id, path: names[r], name: names[r], role: r as string | null, unread: 0, position: null as number | null })),
    { id: a.id * 10 + 7, accountId: a.id, path: "Clients", name: "Clients", role: null, unread: 0, position: null },
    { id: a.id * 10 + 8, accountId: a.id, path: "Receipts", name: "Receipts", role: null, unread: 0, position: null }],
);

const people = [
  ["Maya Chen", "maya@northwind.io"], ["Jordan Blake", "jordan@figment.studio"], ["Stripe", "receipts@stripe.com"],
  ["Priya Nair", "priya@acme.co"], ["Tom Okafor", "tom@okafor.dev"], ["Linear", "notifications@linear.app"],
  ["Sofia Rossi", "sofia@rossi.it"], ["GitHub", "noreply@github.com"], ["Daniel Kim", "dan@kim.vc"],
];
/** Stand-in sender pictures: a BIMI-style logo (fills the avatar) and website icons (on a plate). */
const senderIcons: Record<string, { svg: string; logo: boolean }> = {
  "linear.app": { logo: true, svg: `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><rect width="64" height="64" fill="#5E6AD2"/><path d="M18 40 40 18M22 46 46 22M30 48 48 30" stroke="#fff" stroke-width="5" stroke-linecap="round"/></svg>` },
  "stripe.com": { logo: false, svg: `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32"><rect x="2" y="2" width="28" height="28" rx="7" fill="#635BFF"/><circle cx="16" cy="16" r="6" fill="#fff"/></svg>` },
  "github.com": { logo: false, svg: `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32"><circle cx="16" cy="16" r="14" fill="#1f2328"/><path d="M10 20c3 3 9 3 12 0" stroke="#fff" stroke-width="2.5" fill="none" stroke-linecap="round"/></svg>` },
};

const subjects = [
  ["Q4 planning — draft agenda", "Sharing the agenda for Thursday. Main topics are hiring, roadmap and the offsite."],
  ["Invoice #2041 for November", "Your invoice is attached. Payment is due within 14 days of receipt."],
  ["Re: Homepage copy", "Love the new direction. Two small notes on the hero section before we ship."],
  ["Your receipt from Acme", "Thanks for your payment. Here is a summary of your purchase."],
  ["Coffee next week?", "I'll be in town Tuesday through Thursday, would be great to catch up."],
  ["[mailvern] PR #42 merged", "Merged branch feature/signatures into main."],
  ["Contract v3", "Updated with the changes we discussed. Let me know if anything looks off."],
  ["Weekly metrics", "Signups up 12% week over week. Activation steady at 41%."],
];

const now = Math.floor(Date.now() / 1000);
type M = {
  id: number; accountId: number; folderId: number; fromName: string; fromAddr: string; subject: string; snippet: string;
  date: number; seen: boolean; flagged: boolean; hasAttachments: boolean; threadId: string | null; count: number; ids: number[]; snoozedUntil: number | null;
};
function inScope(m: M, s: any): boolean {
  if (!s || s.kind === "all") return true;
  if (s.kind === "account") return m.accountId === s.accountId;
  if (s.kind === "folder") return m.folderId === s.folderId;
  return folders.find((f) => f.id === m.folderId)?.role === s.role;
}

const messages: M[] = Array.from({ length: 64 }, (_, i) => {
  const p = people[i % people.length];
  const s = subjects[i % subjects.length];
  const acc = accounts[i % 3];
  return {
    id: i + 1,
    accountId: acc.id,
    folderId: acc.id * 10,
    fromName: p[0],
    fromAddr: p[1],
    subject: s[0],
    snippet: s[1],
    date: now - i * 5400 - (i > 6 ? 86400 * Math.floor(i / 4) : 0),
    seen: i % 3 !== 0,
    flagged: i === 2,
    hasAttachments: i % 4 === 1,
    threadId: `t${i + 1}`,
    count: 1,
    ids: [i + 1],
    snoozedUntil: null,
  };
});
// A conversation: Maya's agenda (1), Alex's reply in Sent (65), and Maya again (66).
messages[0].date = now - 7200;
messages.push(
  { ...messages[0], id: 65, folderId: 12, fromName: "Alex Rivera", fromAddr: "hello@acme.co", subject: "Re: Q4 planning — draft agenda", snippet: "Looks good. Can we add 15 minutes for the offsite budget?", date: now - 3000, seen: true, threadId: "t1", hasAttachments: false, flagged: false, ids: [65] },
  { ...messages[0], id: 66, subject: "Re: Q4 planning — draft agenda", snippet: "Done, added it after hiring. See you Thursday!", date: now - 600, seen: false, threadId: "t1", hasAttachments: false, flagged: false, ids: [66] },
  { ...messages[4], id: 67, folderId: 16, subject: "Lunch on Friday?", snippet: "Snoozed until the weekend.", date: now - 86400 * 3, seen: true, threadId: "t67", ids: [67], snoozedUntil: now + 86400 * 2 },
);
const outbox: any[] = [];
let outboxId = 0;

function refreshCounts() {
  for (const f of folders) f.unread = messages.filter((m) => m.folderId === f.id && !m.seen).length;
}
refreshCounts();

const html = `<div style="font-family:-apple-system,Helvetica,sans-serif">
<p>Hi Alex,</p>
<p>Sharing the agenda for Thursday. Main topics:</p>
<ul><li>Hiring plan for Q1</li><li>Roadmap: signatures, search, offline mode</li><li>Offsite logistics</li></ul>
<p>Let me know if you'd like to add anything. The deck is <a href="https://example.com">here</a>.</p>
<p>Thanks,<br>Maya</p>
<img src="https://example.com/pixel.gif" width="1" height="1">
</div>`;

const settings: Record<string, string> = {};
let olderCalls = 0;

// ---- AI: OpenRouter answered by a script, so the agent loop and rephrasing run end to end. ----

function sse(chunks: object[]): Response {
  const enc = new TextEncoder();
  const body = new ReadableStream({
    async start(c) {
      for (const ch of chunks) {
        c.enqueue(enc.encode(`data: ${JSON.stringify({ id: "gen-mock", object: "chat.completion.chunk", created: 0, model: "mock", ...ch })}\n\n`));
        await new Promise((r) => setTimeout(r, 120));
      }
      c.enqueue(enc.encode("data: [DONE]\n\n"));
      c.close();
    },
  });
  return new Response(body, { headers: { "content-type": "text/event-stream" } });
}

const textChunks = (text: string) => text.match(/.{1,18}/gs)!.map((t) => ({ choices: [{ index: 0, delta: { content: t }, finish_reason: null }] }));
const toolChunks = (calls: { name: string; args: object }[]) =>
  calls.map((c, i) => ({ choices: [{ index: 0, delta: { tool_calls: [{ index: i, id: `call_${c.name}_${i}`, type: "function", function: { name: c.name, arguments: JSON.stringify(c.args) } }] }, finish_reason: null }] }));
const end = (reason: string) => ({ choices: [{ index: 0, delta: {}, finish_reason: reason }], usage: { prompt_tokens: 1800, completion_tokens: 120, total_tokens: 1920, cost: 0.0021 } });

function mockOpenRouter(url: string, init?: RequestInit): Response | null {
  if (url.endsWith("/key")) return Response.json({ data: { label: "mock key", usage: 0.42, limit: null } });
  if (url.endsWith("/models")) {
    const m = (id: string, name: string, p: number, c: number) => ({
      id, name, context_length: 1_000_000, architecture: { input_modalities: ["text", "image"], output_modalities: ["text"] },
      pricing: { prompt: String(p / 1e6), completion: String(c / 1e6) }, top_provider: { max_completion_tokens: 64000 }, supported_parameters: ["tools", "reasoning"],
    });
    return Response.json({ data: [m("anthropic/claude-haiku-5.5", "Anthropic: Claude Haiku 5.5", 0.1, 0.5), m("anthropic/claude-sonnet-5.5", "Anthropic: Claude Sonnet 5.5", 2, 10), m("google/gemini-3.8-flash", "Google: Gemini 3.8 Flash", 0.83, 4.12)] });
  }
  if (!url.endsWith("/chat/completions")) return null;
  const req = JSON.parse(String(init?.body ?? "{}"));
  (window as any).__lastAiRequest = req;
  const msgs: any[] = req.messages ?? [];
  const textOf = (c: any): string => (typeof c === "string" ? c : Array.isArray(c) ? c.map((p) => p.text ?? "").join("") : "");
  if (!req.tools) {
    const user = textOf(msgs[msgs.length - 1]?.content);
    const short = user.includes("shorter");
    return sse([...textChunks(short ? "<div>Hi Maya,</div><div><br></div><div>The agenda looks good. Could we add 15 minutes for the offsite budget?</div>" : "<div>Hi Maya,</div><div><br></div><div>Thanks for sharing the agenda. It looks great to me. Could we add 15 minutes to discuss the offsite budget?</div><div><br></div><div>See you Thursday.</div>"), end("stop")]);
  }
  // Script: search, read the conversation it found, then answer. Follow-ups answer at once.
  const lastUser = msgs.map((m) => m.role).lastIndexOf("user");
  const results = msgs.slice(lastUser + 1).filter((m) => m.role === "tool");
  const followUp = msgs.filter((m) => m.role === "user").length > 1;
  if (followUp && results.length === 0) {
    return sse([...toolChunks([{ name: "show_results", args: { answer: "They offered to connect you with their support team for pricing and a technical roadmap instead [1].", sources: [66] } }]), end("tool_calls")]);
  }
  if (results.length === 0) {
    return sse([...toolChunks([{ name: "search_mail", args: { text: "agenda" } }, { name: "search_mail", args: { from: "maya" } }]), end("tool_calls")]);
  }
  if (results.length === 2) {
    return sse([...toolChunks([{ name: "read_conversation", args: { id: 66 } }]), end("tool_calls")]);
  }
  return sse([
    ...toolChunks([{
      name: "show_results",
      args: {
        answer: "Maya added the offsite budget to Thursday's agenda, right after hiring [1].",
        facts: [
          { label: "Meeting", value: "Thursday, Q4 planning [2]" },
          { label: "Topics", value: "Hiring plan, roadmap, offsite logistics [2]" },
          { label: "Added", value: "15 minutes for the offsite budget [1][3]" },
        ],
        details: "- You asked for the budget slot on the 23rd [3].\n- Maya confirmed it the same day [1].",
        sources: [66, 1, 65],
      },
    }]),
    end("tool_calls"),
  ]);
}

function matches(m: M, q: any): boolean {
  const has = (hay: string, needle?: string) => !needle || needle.toLowerCase().split(/\s+/).every((w) => hay.toLowerCase().includes(w));
  return has(`${m.subject} ${m.snippet} ${m.fromName} ${m.fromAddr}`, q.text) && has(m.subject, q.subject) && has(`${m.fromName} ${m.fromAddr}`, q.from)
    && (q.after == null || m.date >= q.after) && (q.before == null || m.date < q.before) && (q.unread == null || m.seen === !q.unread)
    && (q.flagged == null || m.flagged === q.flagged) && (q.hasAttachments == null || m.hasAttachments === q.hasAttachments)
    && (q.accountId == null || m.accountId === q.accountId) && (q.folderId == null || m.folderId === q.folderId)
    && (q.role == null || folders.find((f) => f.id === m.folderId)?.role === q.role);
}

export function installMock() {
  mockWindows("main");
  const realFetch = window.fetch.bind(window);
  window.fetch = async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
    if (url.startsWith("https://openrouter.ai/")) {
      const body = init?.body ?? (input instanceof Request ? await input.clone().text() : undefined);
      const r = mockOpenRouter(url, { ...init, body });
      if (r) return r;
    }
    return realFetch(input, init);
  };
  mockIPC(async (cmd, args: any) => {
    switch (cmd) {
      case "accounts": return accounts;
      case "folders": {
        const order = (id: number) => accounts.findIndex((a) => a.id === id);
        return [...folders].sort((x, y) => order(x.accountId) - order(y.accountId) || (x.position ?? 99) - (y.position ?? 99));
      }
      case "reorder_accounts": {
        const ids: number[] = args.ids;
        accounts.sort((x, y) => ids.indexOf(x.id) - ids.indexOf(y.id));
        return null;
      }
      case "reorder_folders":
        (args.ids as number[]).forEach((id, i) => { const f = folders.find((x) => x.id === id); if (f) f.position = i; });
        return null;
      case "messages": {
        const v = args.view;
        const rows = messages
          .filter((m) => (v.kind === "unified" ? folders.find((f) => f.id === m.folderId)?.role === v.role : m.folderId === v.folderId))
          .sort((a, b) => b.date - a.date);
        if (!args.threaded) return rows.slice(args.offset, args.offset + args.limit).map((m) => ({ ...m, count: 1, ids: [m.id] }));
        const groups = new Map<string, M[]>();
        for (const m of rows) groups.set(m.threadId ?? `m${m.id}`, [...(groups.get(m.threadId ?? `m${m.id}`) ?? []), m]);
        const out = [...groups.values()].map((g) => ({
          ...g[0],
          seen: g.every((m) => m.seen),
          ids: g.map((m) => m.id),
          count: messages.filter((m) => m.threadId === g[0].threadId).length,
        }));
        return out.slice(args.offset, args.offset + args.limit);
      }
      case "thread": {
        const m = messages.find((x) => x.id === args.id)!;
        return messages.filter((x) => x.threadId === m.threadId).sort((a, b) => a.date - b.date);
      }
      case "search_server": {
        await new Promise((r) => setTimeout(r, 900));
        return messages.filter((m) => inScope(m, args.scope) && m.snippet.toLowerCase().includes(args.query.toLowerCase())).slice(0, 5);
      }
      case "archive": case "trash": case "junk": case "not_junk": case "move_to": case "snooze": case "unsnooze": {
        const dest = cmd === "snooze" ? 16 : cmd === "move_to" ? args.folderId : cmd === "archive" ? 13 : cmd === "trash" ? 15 : cmd === "junk" ? 14 : 10;
        for (const id of args.ids) {
          const m = messages.find((x) => x.id === id);
          if (!m) continue;
          const acc = m.accountId * 10;
          m.folderId = cmd === "move_to" ? dest : acc + (dest % 10);
          m.snoozedUntil = cmd === "snooze" ? args.until : null;
          if (cmd === "unsnooze") m.seen = false;
        }
        refreshCounts();
        return null;
      }
      case "set_flagged":
        for (const id of args.ids) { const m = messages.find((x) => x.id === id); if (m) m.flagged = args.flagged; }
        return null;
      case "outbox": return outbox;
      case "send": {
        const id = ++outboxId;
        const sendAt = args.sendAt ?? Math.floor(Date.now() / 1000) + Number(settings.undo_send ?? 10);
        outbox.push({ id, accountId: args.message.accountId, message: args.message, sendAt, error: null });
        if (!args.sendAt) setTimeout(() => { const i = outbox.findIndex((o) => o.id === id); if (i >= 0) outbox.splice(i, 1); }, (sendAt * 1000 - Date.now()));
        return id;
      }
      case "cancel_send": {
        const i = outbox.findIndex((o) => o.id === args.id);
        if (i < 0) throw "Too late, it's on its way";
        const [o] = outbox.splice(i, 1);
        return { message: o.message, attachments: [] };
      }
      case "send_now": { const i = outbox.findIndex((o) => o.id === args.id); if (i >= 0) outbox.splice(i, 1); return null; }
      case "save_draft": await new Promise((r) => setTimeout(r, 300)); return args.message.messageId ?? `<draft-${Date.now()}@mock>`;
      case "delete_draft": return null;
      case "load_older": {
        // Simulate the server returning 30 older messages, twice, then nothing.
        olderCalls++;
        if (olderCalls > 2) return 0;
        await new Promise((r) => setTimeout(r, 600));
        const base = Math.max(...messages.map((m) => m.id));
        const oldest = Math.min(...messages.map((m) => m.date));
        for (let i = 0; i < 30; i++) {
          const p = people[i % people.length], s = subjects[i % subjects.length];
          messages.push({ id: base + i + 1, accountId: 1, folderId: 10, fromName: p[0], fromAddr: p[1], subject: `${s[0]} (older)`, snippet: s[1], date: oldest - (i + 1) * 86400, seen: true, flagged: false, hasAttachments: false, threadId: `o${i}`, count: 1, ids: [base + i + 1], snoozedUntil: null });
        }
        return 30;
      }
      case "search": return messages.filter((m) => inScope(m, args.scope) && (m.subject + m.fromName + m.snippet).toLowerCase().includes(args.query.toLowerCase()));
      case "query_mail": {
        const q = args.query;
        const hits = messages.filter((m) => matches(m, q)).sort((a, b) => (q.oldestFirst ? a.date - b.date : b.date - a.date));
        return { total: hits.length, rows: hits.slice(q.offset ?? 0, (q.offset ?? 0) + (q.limit ?? 25)), serverError: null };
      }
      case "group_mail": {
        const hits = messages.filter((m) => matches(m, args.query));
        const g = new Map<string, any>();
        for (const m of hits) {
          const k = m.fromAddr;
          const e = g.get(k) ?? { key: k, label: m.fromName, count: 0, unread: 0, latest: 0 };
          e.count++; e.unread += m.seen ? 0 : 1; e.latest = Math.max(e.latest, m.date);
          g.set(k, e);
        }
        return { total: hits.length, groups: [...g.values()].sort((a, b) => b.count - a.count) };
      }
      case "summaries": return args.ids.map((id: number) => messages.find((m) => m.id === id)).filter(Boolean);
      case "attachment_content":
        return { kind: "text", filename: "Q4-agenda.pdf", mime: "application/pdf", format: "pdf", text: "--- Page 1 ---\nINVOICE #2041\nAmount due: $1,240.00\nDue: 14 days from receipt", totalChars: 72, offset: 0, parts: 1 };
      case "ai_key": return settings.__ai_key ?? null;
      case "set_ai_key": if (args.key) settings.__ai_key = args.key; else delete settings.__ai_key; return null;
      case "badge_count": return messages.filter((m) => !m.seen).length;
      case "mark_badge_seen": return null;
      case "message": {
        const m = messages.find((x) => x.id === args.id)!;
        return {
          ...m,
          to: [{ name: "Alex Rivera", email: accounts[m.accountId - 1].email }],
          cc: m.id % 2 ? [{ name: "Priya Nair", email: "priya@acme.co" }] : [],
          bcc: [],
          replyTo: [],
          messageId: `<m${m.id}@example.com>`,
          inReplyTo: null,
          references: null,
          html: m.id > 64 ? `<p>${m.snippet}</p>` : html,
          text: null,
          attachments: m.hasAttachments ? [{ index: 0, filename: "Q4-agenda.pdf", mime: "application/pdf", size: 284_000 }, { index: 1, filename: "budget.xlsx", mime: "application/vnd.ms-excel", size: 48_200 }] : [],
        };
      }
      case "mark_read": {
        const v = args.view;
        const hit = (fid: number) => (v.kind === "unified" ? folders.find((f) => f.id === fid)?.role === v.role : fid === v.folderId);
        for (const m of messages) if (hit(m.folderId)) m.seen = true;
        refreshCounts();
        return null;
      }
      case "set_seen":
        for (const id of args.ids) { const m = messages.find((x) => x.id === id); if (m) m.seen = args.seen; }
        refreshCounts();
        return null;
      case "contacts": return people.filter((p) => (p[0] + p[1]).toLowerCase().includes(args.query.toLowerCase())).slice(0, 6).map(([name, email]) => ({ name, email }));
      case "sender_icon": {
        if (settings.sender_icons === "off") return null;
        const icon = senderIcons[args.email.split("@").pop()];
        return icon ? { url: `data:image/svg+xml,${encodeURIComponent(icon.svg)}`, logo: icon.logo } : null;
      }
      case "get_setting": return settings[args.key] ?? null;
      case "set_setting": settings[args.key] = args.value; return null;
      case "save_account": {
        const i = accounts.findIndex((a) => a.id === args.account.id);
        if (i >= 0) accounts[i] = { ...args.account };
        return args.account;
      }
      case "plugin:notification|is_permission_granted": return false;
      case "plugin:notification|request_permission": return "denied";
      case "plugin:event|listen": return 1;
      case "plugin:menu|new": return [1, "menu"];
      case "plugin:menu|popup": (window as any).__lastMenu = args; return null;
      case "agent_access":
      case "set_agent_access":
      case "reset_agent_token": {
        if (cmd === "set_agent_access") settings.mcp_enabled = args.enabled ? "on" : "off";
        if (cmd !== "agent_access") settings.mcp_token = cmd === "reset_agent_token" || !settings.mcp_token ? `mv_${crypto.randomUUID().replaceAll("-", "")}` : settings.mcp_token;
        const enabled = settings.mcp_enabled === "on";
        return { enabled, running: enabled, url: "http://127.0.0.1:47823/mcp", token: enabled ? settings.mcp_token : null, error: null };
      }
      // Confirm dialogs are accepted: the answer is the label of the OK button.
      case "plugin:dialog|message": {
        const b = args.buttons;
        const custom = b && typeof b === "object" ? (Object.values(b)[0] as unknown) : null;
        return Array.isArray(custom) ? custom[0] : typeof custom === "string" ? custom : "Yes";
      }
      case "copy_text": (window as any).__copied = args.text; return navigator.clipboard?.writeText(args.text).catch(() => {});
      default: return null;
    }
  });
}
