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
};

export type Addr = { name: string | null; email: string };

export type Attachment = { index: number; filename: string; mime: string; size: number };

export type Detail = Summary & {
  to: Addr[];
  cc: Addr[];
  replyTo: Addr[];
  messageId: string | null;
  references: string | null;
  html: string | null;
  text: string | null;
  attachments: Attachment[];
};

export type View = { kind: "unified"; role: string } | { kind: "folder"; folderId: number } | { kind: "search" };

export type FileInfo = { path: string; name: string; size: number };

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
};

export type MailEvent =
  | { kind: "changed"; accountId: number }
  | { kind: "status"; accountId: number; state: "syncing" | "idle" | "error"; message: string | null }
  | { kind: "newMail"; accountId: number; from: string; subject: string; count: number };

export const api = {
  accounts: () => invoke<Account[]>("accounts"),
  saveAccount: (account: Account, password: string | null) => invoke<Account>("save_account", { account, password }),
  deleteAccount: (id: number) => invoke<void>("delete_account", { id }),
  reorderAccounts: (ids: number[]) => invoke<void>("reorder_accounts", { ids }),
  folders: () => invoke<Folder[]>("folders"),
  reorderFolders: (accountId: number, ids: number[]) => invoke<void>("reorder_folders", { accountId, ids }),
  messages: (view: View, query: string | null, offset: number, limit: number) =>
    invoke<Summary[]>("messages", { view, query, offset, limit }),
  search: (query: string) => invoke<Summary[]>("search", { query }),
  loadOlder: (view: View) => invoke<number>("load_older", { view }),
  unreadCount: () => invoke<number>("unread_count"),
  message: (id: number) => invoke<Detail>("message", { id }),
  setSeen: (ids: number[], seen: boolean) => invoke<void>("set_seen", { ids, seen }),
  markRead: (view: View) => invoke<void>("mark_read", { view }),
  setFlagged: (ids: number[], flagged: boolean) => invoke<void>("set_flagged", { ids, flagged }),
  archive: (ids: number[]) => invoke<void>("archive", { ids }),
  trash: (ids: number[]) => invoke<void>("trash", { ids }),
  junk: (ids: number[]) => invoke<void>("junk", { ids }),
  notJunk: (ids: number[]) => invoke<void>("not_junk", { ids }),
  send: (message: Outgoing) => invoke<void>("send", { message }),
  syncNow: () => invoke<void>("sync_now"),
  syncView: (view: View) => invoke<void>("sync_view", { view }),
  syncAccount: (accountId: number) => invoke<void>("sync_account", { accountId }),
  getSetting: (key: string) => invoke<string | null>("get_setting", { key }),
  setSetting: (key: string, value: string) => invoke<void>("set_setting", { key, value }),
  openAttachment: (id: number, index: number) => invoke<void>("open_attachment", { id, index }),
  saveAttachment: (id: number, index: number) => invoke<string>("save_attachment", { id, index }),
  fileInfo: (paths: string[]) => invoke<FileInfo[]>("file_info", { paths }),
  attachmentFiles: (id: number) => invoke<FileInfo[]>("attachment_files", { id }),
  contacts: (query: string) => invoke<Addr[]>("contacts", { query }),
  openUrl: (url: string) => invoke<void>("open_url", { url }),
  readImage: (path: string) => invoke<string>("read_image", { path }),
};
