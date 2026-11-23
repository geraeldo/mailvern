import { invoke } from "@tauri-apps/api/core";

export type Account = {
  id: number;
  name: string;
  email: string;
  imapHost: string;
  imapPort: number;
  smtpHost: string;
  smtpPort: number;
  color: string;
  signature: string;
  position: number;
  label: string;
};

export type Folder = {
  id: number;
  accountId: number;
  path: string;
  name: string;
  role: string | null;
  unread: number;
  position: number | null;
};

export type Summary = {
  id: number;
  accountId: number;
  folderId: number;
  fromName: string;
  fromAddr: string;
  subject: string;
  snippet: string;
  date: number;
  seen: boolean;
  flagged: boolean;
  hasAttachments: boolean;
  threadId: string | null;
  /** Messages in the conversation (1 when not grouped). */
  count: number;
  /** Every message this row stands for in the view: one, or a conversation's. */
  ids: number[];
  /** When a snoozed message comes back (unix seconds). */
  snoozedUntil: number | null;
};

export type Addr = { name: string | null; email: string };

export type Attachment = { index: number; filename: string; mime: string; size: number };

export type Detail = Summary & {
  to: Addr[];
  cc: Addr[];
  bcc: Addr[];
  replyTo: Addr[];
  messageId: string | null;
  inReplyTo: string | null;
  references: string | null;
  html: string | null;
  text: string | null;
  attachments: Attachment[];
};

/** "outbox" is the app's own list of queued mail; it never goes to the backend as a view. */
export type View = { kind: "unified"; role: string } | { kind: "folder"; folderId: number } | { kind: "outbox" };

export type MailView = Exclude<View, { kind: "outbox" }>;
/** Where a search looks. */
export type SearchScope =
  | { kind: "all" }
  | { kind: "account"; accountId: number }
  | { kind: "unified"; role: string }
  | { kind: "folder"; folderId: number };

export type FileInfo = { path: string; name: string; size: number };

/** A structured search (AI tools). Every field that is set must match. */
export type MailQuery = {
  /** Words anywhere in subject, sender or body. */
  text?: string;
  subject?: string;
  /** Part of the sender's name or address. */
  from?: string;
  /** Part of a recipient's name or address (To or Cc). */
  to?: string;
  /** Unix seconds, inclusive. */
  after?: number;
  /** Unix seconds, exclusive. */
  before?: number;
  unread?: boolean;
  flagged?: boolean;
  hasAttachments?: boolean;
  accountId?: number;
  folderId?: number;
  role?: string;
  limit?: number;
  offset?: number;
  oldestFirst?: boolean;
};

export type QueryResult = { total: number; rows: Summary[]; serverError: string | null };

export type MailGroup = { key: string; label: string | null; count: number; unread: number; latest: number };

export type AttachmentContent =
  | { kind: "text"; filename: string; mime: string; format: string; text: string; totalChars: number; offset: number; parts: number | null }
  | { kind: "image"; filename: string; mime: string; data: string; size: number }
  | { kind: "unsupported"; filename: string; mime: string; size: number; reason: string };

export type Outgoing = {
  accountId: number;
  to: string[];
  cc: string[];
  bcc: string[];
  subject: string;
  html: string;
  text: string;
  attachments: string[];
  inReplyTo: string | null;
  references: string | null;
  /** Kept across draft saves so each replaces the last; sending it removes the draft. */
  messageId?: string | null;
};

export type OutboxItem = { id: number; accountId: number; message: Outgoing; sendAt: number; error: string | null };

export type Cancelled = { message: Outgoing; attachments: FileInfo[] };

/** Whether coding agents on this Mac may connect (the MCP server), and how. */
export type AgentAccess = { enabled: boolean; running: boolean; url: string; token: string | null; error: string | null };

export type MailEvent =
  | { kind: "changed"; accountId: number }
  | { kind: "status"; accountId: number; state: "syncing" | "idle" | "error"; message: string | null }
  | { kind: "newMail"; accountId: number; from: string; subject: string; count: number }
  | { kind: "outbox" }
  | { kind: "sendFailed"; subject: string; message: string };

export const api = {
  accounts: () => invoke<Account[]>("accounts"),
  saveAccount: (account: Account, password: string | null) => invoke<Account>("save_account", { account, password }),
  deleteAccount: (id: number) => invoke<void>("delete_account", { id }),
  reorderAccounts: (ids: number[]) => invoke<void>("reorder_accounts", { ids }),
  folders: () => invoke<Folder[]>("folders"),
  reorderFolders: (accountId: number, ids: number[]) => invoke<void>("reorder_folders", { accountId, ids }),
  messages: (view: MailView, query: string | null, offset: number, limit: number, threaded = true) =>
    invoke<Summary[]>("messages", { view, query, offset, limit, threaded }),
  thread: (id: number) => invoke<Summary[]>("thread", { id }),
  search: (query: string, scope: SearchScope) => invoke<Summary[]>("search", { query, scope }),
  searchServer: (query: string, scope: SearchScope) => invoke<Summary[]>("search_server", { query, scope }),
  queryMail: (query: MailQuery, server = false) => invoke<QueryResult>("query_mail", { query, server }),
  groupMail: (query: MailQuery, by: string) => invoke<{ total: number; groups: MailGroup[] }>("group_mail", { query, by }),
  summaries: (ids: number[]) => invoke<Summary[]>("summaries", { ids }),
  attachmentContent: (id: number, index: number, offset = 0, maxChars = 20000) =>
    invoke<AttachmentContent>("attachment_content", { id, index, offset, maxChars }),
  aiKey: () => invoke<string | null>("ai_key"),
  setAiKey: (key: string) => invoke<void>("set_ai_key", { key }),
  loadOlder: (view: MailView) => invoke<number>("load_older", { view }),
  badgeCount: () => invoke<number>("badge_count"),
  markBadgeSeen: () => invoke<void>("mark_badge_seen"),
  message: (id: number) => invoke<Detail>("message", { id }),
  setSeen: (ids: number[], seen: boolean) => invoke<void>("set_seen", { ids, seen }),
  markRead: (view: MailView) => invoke<void>("mark_read", { view }),
  setFlagged: (ids: number[], flagged: boolean) => invoke<void>("set_flagged", { ids, flagged }),
  archive: (ids: number[]) => invoke<void>("archive", { ids }),
  trash: (ids: number[]) => invoke<void>("trash", { ids }),
  junk: (ids: number[]) => invoke<void>("junk", { ids }),
  notJunk: (ids: number[]) => invoke<void>("not_junk", { ids }),
  moveTo: (ids: number[], folderId: number) => invoke<void>("move_to", { ids, folderId }),
  snooze: (ids: number[], until: number) => invoke<void>("snooze", { ids, until }),
  unsnooze: (ids: number[]) => invoke<void>("unsnooze", { ids }),
  /** Queues a message (after the undo window, or at `sendAt`). Returns its outbox id. */
  send: (message: Outgoing, sendAt: number | null = null) => invoke<number>("send", { message, sendAt }),
  cancelSend: (id: number) => invoke<Cancelled>("cancel_send", { id }),
  sendNow: (id: number) => invoke<void>("send_now", { id }),
  outbox: () => invoke<OutboxItem[]>("outbox"),
  saveDraft: (message: Outgoing) => invoke<string>("save_draft", { message }),
  deleteDraft: (accountId: number, messageId: string) => invoke<void>("delete_draft", { accountId, messageId }),
  syncNow: () => invoke<void>("sync_now"),
  syncView: (view: MailView) => invoke<void>("sync_view", { view }),
  syncAccount: (accountId: number) => invoke<void>("sync_account", { accountId }),
  getSetting: (key: string) => invoke<string | null>("get_setting", { key }),
  setSetting: (key: string, value: string) => invoke<void>("set_setting", { key, value }),
  openAttachment: (id: number, index: number) => invoke<void>("open_attachment", { id, index }),
  saveAttachment: (id: number, index: number) => invoke<string>("save_attachment", { id, index }),
  copyText: (text: string) => invoke<void>("copy_text", { text }),
  fileInfo: (paths: string[]) => invoke<FileInfo[]>("file_info", { paths }),
  attachmentFiles: (id: number) => invoke<FileInfo[]>("attachment_files", { id }),
  contacts: (query: string) => invoke<Addr[]>("contacts", { query }),
  openUrl: (url: string) => invoke<void>("open_url", { url }),
  readImage: (path: string) => invoke<string>("read_image", { path }),
  agentAccess: () => invoke<AgentAccess>("agent_access"),
  setAgentAccess: (enabled: boolean) => invoke<AgentAccess>("set_agent_access", { enabled }),
  resetAgentToken: () => invoke<AgentAccess>("reset_agent_token"),
};
