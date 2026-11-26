import { invoke } from "@tauri-apps/api/core";

export type Alias = { name: string; email: string };
export type AccountBadgeMode = "" | "new" | "all" | "off";

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
  aliases: Alias[];
  replyTo: string;
  notify: boolean;
  badgeMode: AccountBadgeMode;
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

export type SecurityWarning = { kind: string; severity: "warn" | "danger"; title: string; detail: string };
export type CalendarAttendee = { name: string | null; email: string; status: string };
export type CalendarInvite = {
  uid: string;
  method: string;
  summary: string;
  location: string | null;
  description: string | null;
  start: number | null;
  end: number | null;
  allDay: boolean;
  tzid: string | null;
  dtstartRaw: string;
  dtendRaw: string | null;
  organizer: Addr | null;
  attendees: CalendarAttendee[];
  sequence: number;
  myStatus: string | null;
};

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
  /** The List-Id header of mailing lists and notifications. */
  listId: string | null;
  warnings?: SecurityWarning[];
  calendarInvite?: CalendarInvite | null;
};

export type SmartName = "flagged" | "unread";
export type SmartCounts = { flagged: number; unread: number };
export type BootstrapData = {
  accounts: Account[];
  folders: Folder[];
  smartCounts: SmartCounts;
  messages: Summary[];
  outbox: OutboxItem[];
  pendingAgentActions: number;
  settings: Record<string, string>;
};

/** "outbox" is the app's own list of queued mail; it never goes to the backend as a view. */
export type View =
  | { kind: "unified"; role: string }
  | { kind: "folder"; folderId: number }
  | { kind: "smart"; name: SmartName; accountId?: number | null }
  | { kind: "outbox" };

export type MailView = Exclude<View, { kind: "outbox" }>;
/** Where a search looks. */
export type SearchScope =
  | { kind: "all" }
  | { kind: "account"; accountId: number }
  | { kind: "unified"; role: string }
  | { kind: "folder"; folderId: number };

export type FileInfo = { path: string; name: string; size: number };

/** A sender picture as a data: URL. `logo` (BIMI) fills the avatar; website icons sit on a plate. */
export type SenderIcon = { url: string; logo: boolean };

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
  /** Optional send-as alias email; omitted/null uses the account's primary address. */
  fromEmail?: string | null;
  /** Optional display name override for `fromEmail`. */
  fromName?: string | null;
  /** Optional custom Reply-To address (null/undefined = account default, "" = none). */
  replyTo?: string | null;
};

export type OutboxItem = { id: number; accountId: number; message: Outgoing; sendAt: number; error: string | null };

export type Cancelled = { message: Outgoing; attachments: FileInfo[] };

export type AgentMode = "approval" | "auto";

/** Whether coding agents on this Mac may connect (the MCP server), how, and with what permissions. */
export type AgentAccess = {
  enabled: boolean;
  running: boolean;
  url: string;
  token: string | null;
  error: string | null;
  mode: AgentMode;
  accountModes: Record<number, AgentMode>;
};

export type AgentMessagePreview = { id: number; subject: string; fromName: string; fromAddr: string };
export type AgentActionPreview = {
  messages?: AgentMessagePreview[];
  targetFolder?: string;
  until?: number;
  detail?: string;
};
export type AgentAction = {
  id: number;
  accountId: number | null;
  kind: string;
  summary: string;
  spec: { type: string; [key: string]: unknown };
  preview: AgentActionPreview;
  status: "pending" | "approved" | "rejected" | "failed" | "auto";
  error: string | null;
  createdAt: number;
  resolvedAt: number | null;
};
export type BatchOutcome = { succeeded: number; failed: number; errors: string[] };

export type RuleField =
  | "from"
  | "fromDomain"
  | "to"
  | "cc"
  | "toOrCc"
  | "onlyToMe"
  | "ccMe"
  | "subject"
  | "body"
  | "listId"
  | "hasAttachment";
export type RuleOp = "contains" | "is" | "startsWith" | "endsWith" | "regex";
export type RuleCondition = { field: RuleField; op: RuleOp; value: string };
export type SnoozeUntil = "laterToday" | "tomorrow" | "nextWeek";
export type RuleAction =
  | { type: "move"; folderId: number }
  | { type: "archive" }
  | { type: "trash" }
  | { type: "junk" }
  | { type: "markRead" }
  | { type: "flag" }
  | { type: "snooze"; until: SnoozeUntil }
  | { type: "skipNotification" }
  | { type: "forward"; to: string[] }
  | { type: "reply"; subject: string | null; text: string };
export type Rule = {
  /** 0 for a rule not saved yet. */
  id: number;
  name: string;
  enabled: boolean;
  /** Only mail arriving in this account; null: every account. */
  accountId: number | null;
  /** All conditions must hold, or any one. */
  matchAll: boolean;
  /** None: every new message. */
  conditions: RuleCondition[];
  exceptions: RuleCondition[];
  actions: RuleAction[];
  /** Don't look at later rules once this one matches. */
  stop: boolean;
  origin: "user" | "agent";
  /** An agent added or changed its sending actions: the user has to turn it on. */
  needsApproval: boolean;
  /** Why Mailvern switched it off. */
  pausedReason: string | null;
};
/** The headers rules look at in a message. */
export type RuleFacts = {
  id: number;
  accountId: number;
  folderId: number;
  fromName: string;
  fromAddr: string;
  to: Addr[];
  cc: Addr[];
  replyTo: Addr[];
  subject: string;
  listId: string | null;
  /** Set when a machine sent it: its Auto-Submitted value, or bulk/list/junk from Precedence. */
  auto: string | null;
  hasAttachments: boolean;
  seen: boolean;
  date: number;
  role: string | null;
};
export type RuleTest = { count: number; scanned: number; withoutBody: number; sample: Summary[] };
export type RuleRun = { matched: number; sent: number; notSent: number };
export type RuleActivity = {
  id: number;
  ruleId: number | null;
  ruleName: string;
  accountId: number;
  subject: string;
  fromAddr: string;
  action: RuleAction;
  at: number;
  undone: boolean;
  canUndo: boolean;
};

export type MailEvent =
  | { kind: "changed"; accountId: number; folderId?: number }
  | { kind: "status"; accountId: number; state: "syncing" | "idle" | "error"; message: string | null }
  | { kind: "newMail"; accountId: number; from: string; subject: string; count: number }
  | { kind: "outbox" }
  | { kind: "sendFailed"; subject: string; message: string }
  | { kind: "rulesChanged"; pendingApproval: number }
  | { kind: "rulePaused"; ruleId: number; name: string; reason: string }
  | { kind: "agentActionsChanged"; pending: number }
  | { kind: "agentSent"; outboxId: number; accountId: number; subject: string; to: string[] };

export const api = {
  bootstrap: (view: MailView, limit: number, settingKeys: string[]) =>
    invoke<BootstrapData>("bootstrap", { view, limit, settingKeys }),
  getSettings: (keys: string[]) => invoke<Record<string, string>>("get_settings", { keys }),
  accounts: () => invoke<Account[]>("accounts"),
  saveAccount: (account: Account, password: string | null) => invoke<Account>("save_account", { account, password }),
  deleteAccount: (id: number) => invoke<void>("delete_account", { id }),
  reorderAccounts: (ids: number[]) => invoke<void>("reorder_accounts", { ids }),
  setAccountAlerts: (accountId: number, notify: boolean, badgeMode: AccountBadgeMode) =>
    invoke<void>("set_account_alerts", { accountId, notify, badgeMode }),
  folders: () => invoke<Folder[]>("folders"),
  smartCounts: () => invoke<SmartCounts>("smart_counts"),
  reorderFolders: (accountId: number, ids: number[]) => invoke<void>("reorder_folders", { accountId, ids }),
  createFolder: (accountId: number, name: string, parentId: number | null = null) =>
    invoke<Folder>("create_folder", { accountId, name, parentId }),
  renameFolder: (folderId: number, newName: string) => invoke<void>("rename_folder", { folderId, newName }),
  deleteFolder: (folderId: number) => invoke<void>("delete_folder", { folderId }),
  emptyFolder: (view: MailView) => invoke<void>("empty_folder", { view }),
  permanentDelete: (ids: number[]) => invoke<void>("permanent_delete", { ids }),
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
  rsvpInvite: (id: number, status: "ACCEPTED" | "TENTATIVE" | "DECLINED") => invoke<string>("rsvp_invite", { id, status }),
  openIcs: (id: number) => invoke<void>("open_ics", { id }),
  rawSource: (id: number) => invoke<string>("raw_source", { id }),
  exportEml: (id: number, dest: string | null = null) => invoke<string>("export_eml", { id, dest }),
  cachedMessage: (id: number) => invoke<Detail | null>("cached_message", { id }),
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
  saveAllAttachments: (id: number, destDir: string | null = null) => invoke<number>("save_all_attachments", { id, destDir }),
  quicklookAttachment: (id: number, index: number) => invoke<void>("quicklook_attachment", { id, index }),
  stageAttachment: (id: number, index: number) => invoke<FileInfo>("stage_attachment", { id, index }),
  stageEml: (id: number) => invoke<FileInfo>("stage_eml", { id }),
  copyText: (text: string) => invoke<void>("copy_text", { text }),
  fileInfo: (paths: string[]) => invoke<FileInfo[]>("file_info", { paths }),
  attachmentFiles: (id: number) => invoke<FileInfo[]>("attachment_files", { id }),
  contacts: (query: string) => invoke<Addr[]>("contacts", { query }),
  sentRecipients: () => invoke<string[]>("sent_recipients"),
  /** The sender's domain logo or website icon, null to show initials. */
  senderIcon: (email: string) => invoke<SenderIcon | null>("sender_icon", { email }),
  openUrl: (url: string) => invoke<void>("open_url", { url }),
  readImage: (path: string) => invoke<string>("read_image", { path }),
  agentAccess: () => invoke<AgentAccess>("agent_access"),
  setAgentAccess: (enabled: boolean) => invoke<AgentAccess>("set_agent_access", { enabled }),
  resetAgentToken: () => invoke<AgentAccess>("reset_agent_token"),
  setAgentMode: (mode: AgentMode) => invoke<AgentAccess>("set_agent_mode", { mode }),
  setAgentAccountMode: (accountId: number, mode: AgentMode | null) => invoke<AgentAccess>("set_agent_account_mode", { accountId, mode }),
  agentActions: (limit = 30) => invoke<AgentAction[]>("agent_actions", { limit }),
  pendingAgentActionsCount: () => invoke<number>("pending_agent_actions_count"),
  approveAgentAction: (id: number) => invoke<AgentAction>("approve_agent_action", { id }),
  rejectAgentAction: (id: number) => invoke<void>("reject_agent_action", { id }),
  approveAllAgentActions: () => invoke<BatchOutcome>("approve_all_agent_actions"),
  rejectAllAgentActions: () => invoke<number>("reject_all_agent_actions"),
  rules: () => invoke<Rule[]>("rules"),
  saveRule: (rule: Rule) => invoke<Rule>("save_rule", { rule }),
  deleteRule: (id: number) => invoke<void>("delete_rule", { id }),
  reorderRules: (ids: number[]) => invoke<void>("reorder_rules", { ids }),
  setRuleEnabled: (id: number, enabled: boolean) => invoke<void>("set_rule_enabled", { id, enabled }),
  /** Rejects with what to fix. */
  checkRule: (rule: Rule) => invoke<void>("check_rule", { rule }),
  testRule: (rule: Rule, folderId: number | null = null) => invoke<RuleTest>("test_rule", { rule, folderId }),
  runRule: (id: number, folderId: number | null, includeSending: boolean) =>
    invoke<RuleRun>("run_rule", { id, folderId, includeSending }),
  ruleActivity: (limit = 50) => invoke<RuleActivity[]>("rule_activity", { limit }),
  undoRuleActivity: (id: number) => invoke<void>("undo_rule_activity", { id }),
  ruleFacts: (ids: number[]) => invoke<RuleFacts[]>("rule_facts", { ids }),
};
