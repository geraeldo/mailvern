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

export function installMock() {
  mockWindows("main");
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
      default: return null;
    }
  });
}
