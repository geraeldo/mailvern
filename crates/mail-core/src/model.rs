use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    #[serde(default)]
    pub id: i64,
    pub name: String,
    pub email: String,
    pub imap_host: String,
    pub imap_port: u16,
    pub smtp_host: String,
    pub smtp_port: u16,
    #[serde(default)]
    pub color: String,
    #[serde(default)]
    pub signature: String,
    #[serde(default)]
    pub position: i64,
    /// Short name for the sidebar. Falls back to `name`, then `email`.
    #[serde(default)]
    pub label: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Folder {
    pub id: i64,
    pub account_id: i64,
    pub path: String,
    pub name: String,
    pub role: Option<String>,
    pub unread: i64,
    pub position: Option<i64>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MessageSummary {
    pub id: i64,
    pub account_id: i64,
    pub folder_id: i64,
    pub from_name: String,
    pub from_addr: String,
    pub subject: String,
    pub snippet: String,
    pub date: i64,
    pub seen: bool,
    pub flagged: bool,
    pub has_attachments: bool,
    pub thread_id: Option<String>,
    /// Messages in the conversation (1 when not grouped).
    pub count: i64,
    /// Every message this row stands for in the current view: one, or a conversation's.
    pub ids: Vec<i64>,
    /// When a snoozed message comes back (unix seconds).
    pub snoozed_until: Option<i64>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Addr {
    pub name: Option<String>,
    pub email: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentInfo {
    pub index: usize,
    pub filename: String,
    pub mime: String,
    pub size: usize,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MessageDetail {
    #[serde(flatten)]
    pub summary: MessageSummary,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
    pub bcc: Vec<Addr>,
    pub reply_to: Vec<Addr>,
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Option<String>,
    pub html: Option<String>,
    pub text: Option<String>,
    pub attachments: Vec<AttachmentInfo>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Outgoing {
    pub account_id: i64,
    pub to: Vec<String>,
    #[serde(default)]
    pub cc: Vec<String>,
    #[serde(default)]
    pub bcc: Vec<String>,
    pub subject: String,
    pub html: String,
    pub text: String,
    #[serde(default)]
    pub attachments: Vec<String>,
    pub in_reply_to: Option<String>,
    pub references: Option<String>,
    /// Message-ID to use. A draft keeps one id across saves so each save replaces the last,
    /// and sending it removes the draft.
    #[serde(default)]
    pub message_id: Option<String>,
}

/// A message waiting to be sent (undo window or scheduled), or one that failed.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct OutboxItem {
    pub id: i64,
    pub account_id: i64,
    pub message: Outgoing,
    pub send_at: i64,
    pub error: Option<String>,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum View {
    /// One standard folder (inbox, sent, drafts…) across every account.
    Unified {
        #[serde(default = "inbox_role")]
        role: String,
    },
    Folder {
        #[serde(rename = "folderId")]
        folder_id: i64,
    },
}

fn inbox_role() -> String {
    "inbox".into()
}

/// Folder roles a unified view may cover.
pub const UNIFIED_ROLES: &[&str] = &["inbox", "snoozed", "drafts", "sent", "archive", "junk", "trash"];

#[derive(Serialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Event {
    #[serde(rename_all = "camelCase")]
    Changed { account_id: i64 },
    #[serde(rename_all = "camelCase")]
    Status {
        account_id: i64,
        state: String,
        message: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    NewMail {
        account_id: i64,
        from: String,
        subject: String,
        count: usize,
    },
    /// The outbox changed: something was queued, sent, cancelled or failed.
    Outbox,
    #[serde(rename_all = "camelCase")]
    SendFailed { subject: String, message: String },
}
