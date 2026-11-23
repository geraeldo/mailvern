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
    /// The List-Id header, for "Create rule from this list".
    pub list_id: Option<String>,
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
    /// An `Auto-Submitted` header (RFC 3834) for mail a rule sends: "auto-replied" or "auto-generated".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_submitted: Option<String>,
}

// ---------- rules ----------

/// What part of a message a rule condition looks at.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RuleField {
    /// The sender's name or address.
    From,
    /// The part of the sender's address after "@".
    FromDomain,
    To,
    Cc,
    ToOrCc,
    /// Sent to one of my addresses and nobody else. Ignores `op` and `value`.
    OnlyToMe,
    /// One of my addresses is in Cc. Ignores `op` and `value`.
    CcMe,
    Subject,
    /// The plain text of the body (quoted replies left out).
    Body,
    /// The List-Id header of mailing lists and notifications.
    ListId,
    /// Ignores `op` and `value`.
    HasAttachment,
}

impl RuleField {
    /// Fields that are true or false on their own, without a value to compare.
    pub fn is_flag(self) -> bool {
        matches!(self, RuleField::OnlyToMe | RuleField::CcMe | RuleField::HasAttachment)
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum RuleOp {
    #[default]
    Contains,
    Is,
    StartsWith,
    EndsWith,
    /// A regular expression, case-insensitive.
    Regex,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuleCondition {
    pub field: RuleField,
    #[serde(default)]
    pub op: RuleOp,
    #[serde(default)]
    pub value: String,
}

/// When a snoozed message comes back, relative to when the rule runs.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SnoozeUntil {
    /// In three hours.
    LaterToday,
    /// 8:00 tomorrow.
    Tomorrow,
    /// 8:00 next Monday.
    NextWeek,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum RuleAction {
    #[serde(rename_all = "camelCase")]
    Move { folder_id: i64 },
    Archive,
    Trash,
    Junk,
    MarkRead,
    Flag,
    Snooze { until: SnoozeUntil },
    /// No notification or badge for this message.
    SkipNotification,
    Forward { to: Vec<String> },
    Reply {
        #[serde(default)]
        subject: Option<String>,
        text: String,
    },
}

impl RuleAction {
    /// Actions that send mail on the user's behalf.
    pub fn sends(&self) -> bool {
        matches!(self, RuleAction::Forward { .. } | RuleAction::Reply { .. })
    }

    /// Actions that take the message out of the Inbox. A message can only go to one place.
    pub fn relocates(&self) -> bool {
        matches!(
            self,
            RuleAction::Move { .. } | RuleAction::Archive | RuleAction::Trash | RuleAction::Junk | RuleAction::Snooze { .. }
        )
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Rule {
    #[serde(default)]
    pub id: i64,
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Only mail arriving in this account. None: every account.
    #[serde(default)]
    pub account_id: Option<i64>,
    /// All conditions must hold (true) or any one of them (false).
    #[serde(default = "yes")]
    pub match_all: bool,
    /// No conditions: every new message.
    #[serde(default)]
    pub conditions: Vec<RuleCondition>,
    /// The rule doesn't apply when any of these holds.
    #[serde(default)]
    pub exceptions: Vec<RuleCondition>,
    pub actions: Vec<RuleAction>,
    /// Don't look at later rules once this one matches.
    #[serde(default)]
    pub stop: bool,
    /// Who saved it last: "user" or "agent". Set by the engine.
    #[serde(default = "user_origin")]
    pub origin: String,
    /// An agent added or changed mail-sending actions; the user has to turn it on.
    #[serde(default)]
    pub needs_approval: bool,
    /// Why the engine switched it off (e.g. it sent too much mail).
    #[serde(default)]
    pub paused_reason: Option<String>,
}

fn yes() -> bool {
    true
}

fn user_origin() -> String {
    "user".into()
}

/// How a rule would do on mail already here.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RuleTest {
    /// Messages that match.
    pub count: usize,
    /// Messages looked at.
    pub scanned: usize,
    /// Messages that couldn't be checked because their body isn't downloaded.
    pub without_body: usize,
    /// The newest matches.
    pub sample: Vec<MessageSummary>,
}

/// What running a rule on existing mail did.
#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct RuleRun {
    pub matched: usize,
    /// Forwards and replies sent.
    pub sent: usize,
    /// Forwards and replies left out: automatic mail, lists, limits, or not asked for.
    pub not_sent: usize,
}

/// One thing a rule did, for the activity log.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RuleActivity {
    pub id: i64,
    pub rule_id: Option<i64>,
    pub rule_name: String,
    pub account_id: i64,
    pub subject: String,
    pub from_addr: String,
    pub action: RuleAction,
    pub at: i64,
    pub undone: bool,
    pub can_undo: bool,
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

/// Where a search looks.
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SearchScope {
    /// Every folder of every account.
    #[default]
    All,
    /// Every folder of one account.
    Account {
        #[serde(rename = "accountId")]
        account_id: i64,
    },
    /// One standard folder across every account.
    Unified { role: String },
    Folder {
        #[serde(rename = "folderId")]
        folder_id: i64,
    },
}

/// A structured mail search: every field that is set must match. Used by the AI tools.
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct MailQuery {
    /// Words anywhere in the subject, sender or body (prefix match; all must appear).
    pub text: Option<String>,
    /// Words in the subject only.
    pub subject: Option<String>,
    /// Part of the sender's name or address.
    pub from: Option<String>,
    /// Part of a recipient's name or address (To or Cc).
    pub to: Option<String>,
    /// Unix seconds, inclusive.
    pub after: Option<i64>,
    /// Unix seconds, exclusive.
    pub before: Option<i64>,
    pub unread: Option<bool>,
    pub flagged: Option<bool>,
    pub has_attachments: Option<bool>,
    pub account_id: Option<i64>,
    pub folder_id: Option<i64>,
    /// A standard folder: inbox, sent, drafts, archive, junk, trash, snoozed, all.
    pub role: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    /// Oldest first instead of newest first.
    pub oldest_first: bool,
    /// Messages the mail server matched; they count as matching `text`, `subject`, `from` and `to`.
    #[serde(skip)]
    pub server_hits: Vec<i64>,
}

/// One page of matches and how many there are in all.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MailQueryResult {
    pub total: i64,
    pub rows: Vec<MessageSummary>,
}

/// How many messages match, per group.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MailGroup {
    pub key: String,
    /// A readable name for the key, when it has one (a sender's name, a folder's account).
    pub label: Option<String>,
    pub count: i64,
    pub unread: i64,
    /// Newest message in the group (unix seconds).
    pub latest: i64,
}

/// What an attachment holds, made readable for a language model.
#[derive(Serialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AttachmentContent {
    #[serde(rename_all = "camelCase")]
    Text {
        filename: String,
        mime: String,
        /// How the text was obtained, e.g. "pdf", "docx", "spreadsheet".
        format: String,
        text: String,
        /// Characters in the whole text; `text` is the part from `offset`.
        total_chars: usize,
        offset: usize,
        /// Pages, sheets or slides, where the format has them.
        parts: Option<usize>,
    },
    #[serde(rename_all = "camelCase")]
    Image { filename: String, mime: String, data: String, size: usize },
    #[serde(rename_all = "camelCase")]
    Unsupported { filename: String, mime: String, size: usize, reason: String },
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
    /// Rules were added, changed or removed.
    #[serde(rename_all = "camelCase")]
    RulesChanged { pending_approval: usize },
    /// The engine switched a rule off.
    #[serde(rename_all = "camelCase")]
    RulePaused { rule_id: i64, name: String, reason: String },
}
