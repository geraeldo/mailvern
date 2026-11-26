use crate::icons::Icon;
use crate::model::*;
use anyhow::{anyhow, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::Mutex;

pub type SummaryDetailRow = (MessageSummary, Vec<Addr>, Vec<Addr>, Vec<Addr>, Option<String>, Option<String>, Option<String>);
pub type OutboxSummaryRow = (i64, i64, String, i64, Option<String>);

pub struct Db {
    conn: Mutex<Connection>,
    read: Mutex<Connection>,
}

/// Persisted IMAP sync watermarks for a folder.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FolderSyncState {
    pub uidvalidity: u32,
    pub synced: bool,
    pub uidnext: u32,
    /// Last known server EXISTS count, or -1 if unknown/invalidated.
    pub msg_count: i64,
    /// Last known server UNSEEN count, or -1 if unknown.
    pub unseen_count: i64,
    /// Last known CONDSTORE HIGHESTMODSEQ (0 if unsupported/unknown).
    pub highest_modseq: u64,
    /// Unix timestamp of the last full flag reconciliation.
    pub last_full_sync: i64,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum PendingOpPayload {
    Store { flag: String, add: bool },
    MarkAllRead,
    MoveRole { role: String },
    MoveFolder { to_folder_id: i64 },
    Delete,
    EmptyFolder,
}

#[derive(Clone, Debug)]
pub struct PendingOp {
    pub id: i64,
    pub folder_id: i64,
    pub payload: PendingOpPayload,
    pub uids: Vec<u32>,
    pub attempts: i64,
}

/// Header-level data for a message, as stored after a sync.
pub struct NewMessage {
    pub uid: u32,
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub refs: Option<String>,
    pub subject: String,
    pub from_name: String,
    pub from_addr: String,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
    pub reply_to: Vec<Addr>,
    pub date: i64,
    pub seen: bool,
    pub flagged: bool,
    pub has_attachments: bool,
    pub size: u32,
    /// The List-Id header of mailing lists and notifications.
    pub list_id: Option<String>,
    /// Set when the message says it was sent by a machine: its Auto-Submitted value, or
    /// "bulk", "list" or "junk" from Precedence. Rules never auto-reply to these.
    pub auto: Option<String>,
}

/// Summary fields computed from a full message body.
pub struct BodyInfo {
    pub snippet: String,
    pub search_text: String,
    pub has_attachments: bool,
}

/// Per-connection settings. Not part of the schema; applied on every open.
const PRAGMAS: &str = "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;";

/// Caches that can be rebuilt from the network at any time, so they stay out of the
/// versioned schema: created on open, and dropping one only costs a refetch.
///
/// `sender_icons`: one row per sender domain. `mime` is NULL when the domain has no
/// picture. `retry_at` is when to look again.
const CACHES: &str = r#"
CREATE TABLE IF NOT EXISTS sender_icons (
  domain TEXT PRIMARY KEY,
  mime TEXT,
  data BLOB,
  logo INTEGER NOT NULL DEFAULT 0,
  retry_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS moved_bodies (
  account_id INTEGER NOT NULL,
  message_id TEXT NOT NULL,
  raw BLOB NOT NULL,
  snippet TEXT NOT NULL DEFAULT '',
  search_text TEXT NOT NULL DEFAULT '',
  has_attachments INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (account_id, message_id)
);
"#;

/// Schema history. `PRAGMA user_version` records how many of these have run.
///
/// Rules: never edit or reorder a shipped entry. To change the schema, append a new
/// entry (e.g. `ALTER TABLE messages ADD COLUMN ...`). Each one runs once, in a
/// transaction, and the database is backed up before any upgrade.
const MIGRATIONS: &[&str] = &[V1, V2, V3, V4, V5, V6, V7, V8, V9];

/// v9: send-as aliases and default Reply-To per account, flagged message index for smart views,
/// and persisted calendar invite (ICS) RSVP responses.
const V9: &str = r#"
ALTER TABLE accounts ADD COLUMN aliases_json TEXT NOT NULL DEFAULT '[]';
ALTER TABLE accounts ADD COLUMN reply_to TEXT NOT NULL DEFAULT '';

CREATE INDEX IF NOT EXISTS idx_messages_flagged ON messages(flagged) WHERE flagged = 1;

CREATE TABLE IF NOT EXISTS ics_rsvps (
  account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  event_uid TEXT NOT NULL,
  status TEXT NOT NULL,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (account_id, event_uid)
);
"#;

/// v8: folder sync watermarks (UIDNEXT/EXISTS/UNSEEN/HIGHESTMODSEQ), durable pending IMAP ops,
/// quiet flag for SkipNotification badge accuracy, and sync/query indexes.
const V8: &str = r#"
ALTER TABLE folders ADD COLUMN uidnext INTEGER NOT NULL DEFAULT 0;
ALTER TABLE folders ADD COLUMN msg_count INTEGER NOT NULL DEFAULT -1;
ALTER TABLE folders ADD COLUMN unseen_count INTEGER NOT NULL DEFAULT -1;
ALTER TABLE folders ADD COLUMN highest_modseq INTEGER NOT NULL DEFAULT 0;
ALTER TABLE folders ADD COLUMN last_full_sync INTEGER NOT NULL DEFAULT 0;
ALTER TABLE messages ADD COLUMN quiet INTEGER NOT NULL DEFAULT 0;

CREATE TABLE IF NOT EXISTS pending_ops (
  id INTEGER PRIMARY KEY,
  account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  folder_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  payload TEXT NOT NULL,
  uids_json TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  attempts INTEGER NOT NULL DEFAULT 0,
  next_retry INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_pending_ops_acc ON pending_ops(account_id, id);
CREATE INDEX IF NOT EXISTS idx_pending_ops_folder ON pending_ops(folder_id);

CREATE INDEX IF NOT EXISTS idx_messages_folder_seen ON messages(folder_id, seen);
CREATE INDEX IF NOT EXISTS idx_messages_folder_sparse_uid ON messages(folder_id, sparse, uid);
CREATE INDEX IF NOT EXISTS idx_messages_missing_body ON messages(folder_id, date DESC) WHERE has_body = 0;
CREATE INDEX IF NOT EXISTS idx_messages_acc_msgid ON messages(account_id, message_id);
"#;

/// v7: per-account MCP permission overrides and the pending/recent agent actions queue.
const V7: &str = r#"
CREATE TABLE IF NOT EXISTS agent_account_modes (
  account_id INTEGER PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
  mode TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS agent_actions (
  id INTEGER PRIMARY KEY,
  account_id INTEGER REFERENCES accounts(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  summary TEXT NOT NULL,
  payload TEXT NOT NULL,
  preview TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'pending',
  error TEXT,
  created_at INTEGER NOT NULL,
  resolved_at INTEGER
);
CREATE INDEX IF NOT EXISTS idx_agent_actions_status ON agent_actions(status, created_at DESC);
"#;

/// v6: track whether each folder has completed an initial sync, so an emptied Inbox
/// (Inbox Zero) still runs rules and notifies when new mail arrives.
const V6: &str = r#"
ALTER TABLE folders ADD COLUMN synced INTEGER NOT NULL DEFAULT 0;
UPDATE folders SET synced = 1 WHERE uidvalidity != 0 OR EXISTS (SELECT 1 FROM messages m WHERE m.folder_id = folders.id);
"#;

/// v5: rules.
///
/// `list_id` and `auto` come from headers, for rule conditions and loop protection.
/// `rule_queue` holds new Inbox mail waiting for the rule runner. `rule_seen` records the
/// Message-IDs rules have looked at, so a message that comes back to the Inbox (with a new
/// UID) isn't processed twice. `rule_replies` limits auto-replies to one per sender now and then.
const V5: &str = r#"
ALTER TABLE messages ADD COLUMN list_id TEXT;
ALTER TABLE messages ADD COLUMN auto TEXT;

CREATE TABLE IF NOT EXISTS rules (
  id INTEGER PRIMARY KEY,
  position INTEGER NOT NULL DEFAULT 0,
  enabled INTEGER NOT NULL DEFAULT 1,
  def TEXT NOT NULL,
  origin TEXT NOT NULL DEFAULT 'user',
  needs_approval INTEGER NOT NULL DEFAULT 0,
  paused_reason TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS rule_queue (
  message_id INTEGER PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
  account_id INTEGER NOT NULL,
  queued_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS rule_seen (
  account_id INTEGER NOT NULL,
  msgid TEXT NOT NULL,
  at INTEGER NOT NULL,
  PRIMARY KEY (account_id, msgid)
);

CREATE TABLE IF NOT EXISTS rule_log (
  id INTEGER PRIMARY KEY,
  rule_id INTEGER,
  rule_name TEXT NOT NULL,
  account_id INTEGER NOT NULL,
  msgid TEXT,
  subject TEXT NOT NULL DEFAULT '',
  from_addr TEXT NOT NULL DEFAULT '',
  action TEXT NOT NULL,
  at INTEGER NOT NULL,
  undone INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_rule_log_rule ON rule_log(rule_id, at);

CREATE TABLE IF NOT EXISTS rule_replies (
  rule_id INTEGER NOT NULL,
  addr TEXT NOT NULL,
  at INTEGER NOT NULL,
  PRIMARY KEY (rule_id, addr)
);
"#;

/// v4: conversations, the outbox (undo send, send later), snoozes, and server search hits.
///
/// `thread_id` is filled in by `assign_threads` (also for rows that predate it).
/// `sparse` marks a message stored only because a server search found it. Those sit
/// below the contiguous window of recent mail that sync keeps in step with the server,
/// so the window's bounds ignore them.
const V4: &str = r#"
ALTER TABLE messages ADD COLUMN thread_id TEXT;
ALTER TABLE messages ADD COLUMN sparse INTEGER NOT NULL DEFAULT 0;
CREATE INDEX IF NOT EXISTS idx_messages_thread ON messages(account_id, thread_id);

CREATE TABLE IF NOT EXISTS outbox (
  id INTEGER PRIMARY KEY,
  account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  message_id TEXT NOT NULL,
  payload TEXT NOT NULL,
  raw BLOB NOT NULL,
  envelope TEXT NOT NULL,
  send_at INTEGER NOT NULL,
  attempts INTEGER NOT NULL DEFAULT 0,
  sending INTEGER NOT NULL DEFAULT 0,
  error TEXT
);

CREATE TABLE IF NOT EXISTS snoozes (
  id INTEGER PRIMARY KEY,
  account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  message_id TEXT NOT NULL,
  return_path TEXT NOT NULL,
  wake_at INTEGER NOT NULL,
  woke_at INTEGER,
  UNIQUE(account_id, message_id)
);
"#;

/// v3: a user-chosen folder order. NULL means "not placed yet": default order applies.
const V3: &str = "ALTER TABLE folders ADD COLUMN position INTEGER;";

/// v2: a sidebar label separate from the sender name.
const V2: &str = "ALTER TABLE accounts ADD COLUMN label TEXT NOT NULL DEFAULT '';";

const V1: &str = r#"
CREATE TABLE IF NOT EXISTS accounts (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  email TEXT NOT NULL UNIQUE,
  imap_host TEXT NOT NULL,
  imap_port INTEGER NOT NULL,
  smtp_host TEXT NOT NULL,
  smtp_port INTEGER NOT NULL,
  color TEXT NOT NULL DEFAULT '',
  signature TEXT NOT NULL DEFAULT '',
  position INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS folders (
  id INTEGER PRIMARY KEY,
  account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  path TEXT NOT NULL,
  name TEXT NOT NULL,
  role TEXT,
  uidvalidity INTEGER NOT NULL DEFAULT 0,
  UNIQUE(account_id, path)
);

CREATE TABLE IF NOT EXISTS messages (
  id INTEGER PRIMARY KEY,
  account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  folder_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
  uid INTEGER NOT NULL,
  message_id TEXT,
  in_reply_to TEXT,
  refs TEXT,
  subject TEXT NOT NULL DEFAULT '',
  from_name TEXT NOT NULL DEFAULT '',
  from_addr TEXT NOT NULL DEFAULT '',
  to_json TEXT NOT NULL DEFAULT '[]',
  cc_json TEXT NOT NULL DEFAULT '[]',
  reply_to_json TEXT NOT NULL DEFAULT '[]',
  date INTEGER NOT NULL DEFAULT 0,
  snippet TEXT NOT NULL DEFAULT '',
  seen INTEGER NOT NULL DEFAULT 0,
  flagged INTEGER NOT NULL DEFAULT 0,
  has_attachments INTEGER NOT NULL DEFAULT 0,
  size INTEGER NOT NULL DEFAULT 0,
  has_body INTEGER NOT NULL DEFAULT 0,
  UNIQUE(folder_id, uid)
);
CREATE INDEX IF NOT EXISTS idx_messages_folder_date ON messages(folder_id, date DESC);
CREATE INDEX IF NOT EXISTS idx_messages_msgid ON messages(message_id);

CREATE TABLE IF NOT EXISTS bodies (
  message_id INTEGER PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
  raw BLOB NOT NULL
);

CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(subject, sender, body, tokenize = 'unicode61 remove_diacritics 2');

CREATE TRIGGER IF NOT EXISTS messages_fts_delete AFTER DELETE ON messages BEGIN
  DELETE FROM messages_fts WHERE rowid = old.id;
END;

CREATE TABLE IF NOT EXISTS settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
"#;

const SUMMARY_COLS: &str =
    "m.id, m.account_id, m.folder_id, m.from_name, m.from_addr, m.subject, m.snippet, m.date, m.seen, m.flagged, m.has_attachments,
     m.thread_id,
     (SELECT s.wake_at FROM snoozes s WHERE s.account_id = m.account_id AND s.message_id = m.message_id AND s.woke_at IS NULL)";

/// Number of columns in SUMMARY_COLS.
const SUMMARY_LEN: usize = 13;

/// When a message was last "fresh": its date, or when it came back from snooze.
const SORT_DATE: &str =
    "MAX(m.date, COALESCE((SELECT s.woke_at FROM snoozes s WHERE s.account_id = m.account_id AND s.message_id = m.message_id), 0))";

fn row_to_summary(r: &rusqlite::Row) -> rusqlite::Result<MessageSummary> {
    let id: i64 = r.get(0)?;
    Ok(MessageSummary {
        id,
        account_id: r.get(1)?,
        folder_id: r.get(2)?,
        from_name: r.get(3)?,
        from_addr: r.get(4)?,
        subject: r.get(5)?,
        snippet: r.get(6)?,
        date: r.get(7)?,
        seen: r.get(8)?,
        flagged: r.get(9)?,
        has_attachments: r.get(10)?,
        thread_id: r.get(11)?,
        count: 1,
        ids: vec![id],
        snoozed_until: r.get(12)?,
    })
}

/// Message-ids in a References / In-Reply-To value, in order: "<a> <b>" → ["<a>", "<b>"].
fn ids_in(v: Option<&str>) -> Vec<String> {
    let Some(v) = v else { return vec![] };
    v.split('<')
        .filter_map(|p| p.split_once('>').map(|(id, _)| id.trim()))
        .filter(|id| !id.is_empty())
        .map(|id| format!("<{id}>"))
        .collect()
}

/// Puts one stored message into a conversation.
///
/// It joins the conversation of the nearest message it refers to that we already have.
/// Otherwise it starts one named after the root it refers to (or itself). Messages that
/// arrived earlier and named this one as their root then move into its conversation, so
/// the order mail arrives in doesn't matter.
fn assign_thread(
    c: &Connection,
    account_id: i64,
    row_id: i64,
    message_id: Option<&str>,
    in_reply_to: Option<&str>,
    refs: Option<&str>,
) -> rusqlite::Result<()> {
    let mut cands = ids_in(refs);
    for id in ids_in(in_reply_to) {
        if !cands.contains(&id) {
            cands.push(id);
        }
    }
    cands.retain(|id| Some(id.as_str()) != message_id);
    let mut thread: Option<String> = None;
    // Nearest parent first: the last reference is the message being replied to.
    for id in cands.iter().rev() {
        thread = c
            .query_row(
                "SELECT thread_id FROM messages WHERE account_id = ? AND message_id = ? AND thread_id IS NOT NULL LIMIT 1",
                params![account_id, id],
                |r| r.get(0),
            )
            .optional()?;
        if thread.is_some() {
            break;
        }
    }
    let thread = thread
        .or_else(|| cands.first().cloned())
        .or_else(|| message_id.map(String::from))
        .unwrap_or_else(|| format!("m{row_id}"));
    c.execute("UPDATE messages SET thread_id = ? WHERE id = ?", params![thread, row_id])?;
    if let Some(mid) = message_id {
        if mid != thread {
            c.execute(
                "UPDATE messages SET thread_id = ?1 WHERE account_id = ?2 AND thread_id = ?3",
                params![thread, account_id, mid],
            )?;
        }
    }
    Ok(())
}

/// The scope of a view as SQL over `messages m`.
fn scope_sql(view: &View) -> Result<String> {
    Ok(match view {
        View::Unified { role } => {
            let role = UNIFIED_ROLES.iter().find(|r| **r == role).ok_or_else(|| anyhow!("Unknown folder role"))?;
            format!("m.folder_id IN (SELECT id FROM folders WHERE role = '{role}')")
        }
        View::Folder { folder_id } => format!("m.folder_id = {}", folder_id),
        View::Smart { name, account_id } => {
            let acc = match account_id {
                Some(id) => format!(" AND m.account_id = {id}"),
                None => String::new(),
            };
            let dedup = "(m.message_id IS NULL OR m.id = (SELECT d.id FROM messages d JOIN folders df ON df.id = d.folder_id \
                         WHERE d.account_id = m.account_id AND d.message_id = m.message_id \
                         AND (df.role IS NULL OR df.role NOT IN ('trash', 'junk')) \
                         ORDER BY df.role IS 'all', d.id LIMIT 1))";
            match name.as_str() {
                "flagged" => format!(
                    "m.flagged = 1 AND m.folder_id NOT IN (SELECT id FROM folders WHERE role IN ('trash', 'junk')){acc} AND {dedup}"
                ),
                "unread" => format!(
                    "m.seen = 0 AND m.folder_id NOT IN (SELECT id FROM folders WHERE role IN ('trash', 'junk', 'drafts', 'sent')){acc} AND {dedup}"
                ),
                _ => return Err(anyhow!("Unknown smart view {name:?}")),
            }
        }
    })
}

/// A message held in the outbox.
pub struct OutboxRow {
    pub id: i64,
    pub account_id: i64,
    pub message_id: String,
    pub payload: String,
    pub raw: Vec<u8>,
    pub envelope: String,
    pub attempts: i64,
}

/// A snooze whose time has come.
pub struct DueSnooze {
    pub id: i64,
    pub account_id: i64,
    pub message_id: String,
    pub return_path: String,
}

fn row_to_account(r: &rusqlite::Row) -> rusqlite::Result<Account> {
    let aliases_json: String = r.get(11)?;
    Ok(Account {
        id: r.get(0)?,
        name: r.get(1)?,
        email: r.get(2)?,
        imap_host: r.get(3)?,
        imap_port: r.get(4)?,
        smtp_host: r.get(5)?,
        smtp_port: r.get(6)?,
        color: r.get(7)?,
        signature: r.get(8)?,
        position: r.get(9)?,
        label: r.get(10)?,
        aliases: serde_json::from_str(&aliases_json).unwrap_or_default(),
        reply_to: r.get(12)?,
    })
}

/// Turns free text into a safe FTS5 prefix query.
fn fts_query(q: &str) -> Option<String> {
    let terms: Vec<String> = q
        .split_whitespace()
        .map(|t| t.replace(['"', '\0'], ""))
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{}\"*", t))
        .collect();
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" "))
    }
}

/// A LIKE pattern matching `s` anywhere (with `ESCAPE '\'`).
fn like_any(s: &str) -> String {
    format!("%{}%", s.trim().replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"))
}

/// The WHERE clause (over `messages m`) and its parameters for a structured query.
fn query_where(q: &MailQuery) -> Result<(String, Vec<rusqlite::types::Value>)> {
    use rusqlite::types::Value;
    let mut args: Vec<Value> = Vec::new();
    let mut arg = |v: Value| {
        args.push(v);
        format!("?{}", args.len())
    };
    let nonempty = |s: &Option<String>| s.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);

    // Conditions on words, sender and recipients: a server hit already matched these.
    let mut words = Vec::new();
    let mut fts = Vec::new();
    if let Some(t) = q.text.as_deref().and_then(fts_query) {
        fts.push(t);
    }
    if let Some(t) = q.subject.as_deref().and_then(fts_query) {
        fts.push(format!("subject : ({t})"));
    }
    if !fts.is_empty() {
        let p = arg(Value::Text(fts.join(" AND ")));
        words.push(format!("m.id IN (SELECT rowid FROM messages_fts WHERE messages_fts MATCH {p})"));
    }
    if let Some(f) = nonempty(&q.from) {
        let p = arg(Value::Text(like_any(&f)));
        words.push(format!("(m.from_name LIKE {p} ESCAPE '\\' OR m.from_addr LIKE {p} ESCAPE '\\')"));
    }
    if let Some(t) = nonempty(&q.to) {
        let p = arg(Value::Text(like_any(&t)));
        words.push(format!(
            "(EXISTS (SELECT 1 FROM json_each(m.to_json) j WHERE json_extract(j.value, '$.email') LIKE {p} ESCAPE '\\' OR COALESCE(json_extract(j.value, '$.name'), '') LIKE {p} ESCAPE '\\') \
              OR EXISTS (SELECT 1 FROM json_each(m.cc_json) j WHERE json_extract(j.value, '$.email') LIKE {p} ESCAPE '\\' OR COALESCE(json_extract(j.value, '$.name'), '') LIKE {p} ESCAPE '\\'))"
        ));
    }

    let mut w = Vec::new();
    if !words.is_empty() {
        let all = words.join(" AND ");
        if q.server_hits.is_empty() {
            w.push(all);
        } else {
            let ids: Vec<String> = q.server_hits.iter().map(|i| i.to_string()).collect();
            w.push(format!("(({all}) OR m.id IN ({}))", ids.join(",")));
        }
    }
    if let Some(t) = q.after {
        w.push(format!("m.date >= {}", arg(Value::Integer(t))));
    }
    if let Some(t) = q.before {
        w.push(format!("m.date < {}", arg(Value::Integer(t))));
    }
    if let Some(u) = q.unread {
        w.push(format!("m.seen = {}", if u { 0 } else { 1 }));
    }
    if let Some(f) = q.flagged {
        w.push(format!("m.flagged = {}", f as i32));
    }
    if let Some(a) = q.has_attachments {
        w.push(format!("m.has_attachments = {}", a as i32));
    }
    if let Some(a) = q.account_id {
        w.push(format!("m.account_id = {}", arg(Value::Integer(a))));
    }
    if let Some(f) = q.folder_id {
        w.push(format!("m.folder_id = {}", arg(Value::Integer(f))));
    }
    if let Some(role) = nonempty(&q.role) {
        let role = role.to_ascii_lowercase();
        if role != "all" && !UNIFIED_ROLES.contains(&role.as_str()) {
            return Err(anyhow!("Unknown folder role {role:?}: use inbox, sent, drafts, archive, junk, trash, snoozed or all"));
        }
        w.push(format!("m.folder_id IN (SELECT id FROM folders WHERE role = {})", arg(Value::Text(role))));
    }
    // The same message can sit in several folders (Gmail keeps everything in All Mail too).
    // Without a folder filter, count it once, preferring a folder other than All Mail.
    if q.folder_id.is_none() && nonempty(&q.role).is_none() {
        w.push(
            "(m.message_id IS NULL OR m.id = (SELECT d.id FROM messages d JOIN folders df ON df.id = d.folder_id
              WHERE d.account_id = m.account_id AND d.message_id = m.message_id ORDER BY df.role IS 'all', d.id LIMIT 1))"
                .into(),
        );
    }
    Ok((if w.is_empty() { "1".into() } else { w.join(" AND ") }, args))
}

#[cfg(test)]
const SCHEMA_VERSION: i64 = MIGRATIONS.len() as i64;

fn migrate(conn: &mut Connection, path: &Path) -> Result<()> {
    migrate_with(conn, path, MIGRATIONS)
}

fn migrate_with(conn: &mut Connection, path: &Path, migrations: &[&str]) -> Result<()> {
    let latest = migrations.len() as i64;
    let current: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if current > latest {
        return Err(anyhow!(
            "This database was created by a newer version of Mailvern (schema {current}, this app supports {latest}). Update Mailvern to open it."
        ));
    }
    if current == latest {
        return Ok(());
    }
    // Back up an existing database before changing it. A brand-new file has no tables yet.
    let has_tables: bool =
        conn.query_row("SELECT COUNT(*) > 0 FROM sqlite_master WHERE type = 'table'", [], |r| r.get(0))?;
    if has_tables {
        let backup = path.with_extension(format!("v{current}.bak"));
        let _ = std::fs::remove_file(&backup);
        conn.execute("VACUUM INTO ?", [backup.to_string_lossy()])?;
    }
    for (i, sql) in migrations.iter().enumerate().skip(current as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql).with_context(|| format!("Database upgrade to version {} failed", i + 1))?;
        tx.pragma_update(None, "user_version", (i + 1) as i64)?;
        tx.commit()?;
    }
    Ok(())
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let mut conn = Connection::open(path)?;
        conn.execute_batch(PRAGMAS)?;
        migrate(&mut conn, path)?;
        conn.execute_batch(CACHES)?;
        let read = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        read.execute_batch("PRAGMA busy_timeout = 5000; PRAGMA query_only = ON;")?;
        let db = Db { conn: Mutex::new(conn), read: Mutex::new(read) };
        db.assign_threads()?;
        Ok(db)
    }

    /// Gives every message without a conversation one, oldest first (needed once after v4).
    fn assign_threads(&self) -> Result<()> {
        type ThreadSeedRow = (i64, i64, Option<String>, Option<String>, Option<String>);
        let mut c = self.c();
        let tx = c.transaction()?;
        {
            let rows: Vec<ThreadSeedRow> = tx
                .prepare("SELECT id, account_id, message_id, in_reply_to, refs FROM messages WHERE thread_id IS NULL ORDER BY date, id")?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
                .collect::<rusqlite::Result<_>>()?;
            for (id, acc, mid, irt, refs) in rows {
                assign_thread(&tx, acc, id, mid.as_deref(), irt.as_deref(), refs.as_deref())?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    fn c(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Dedicated read-only connection so UI queries run concurrently with sync writes under WAL.
    fn r(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.read.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ---------- accounts ----------

    pub fn accounts(&self) -> Result<Vec<Account>> {
        let c = self.r();
        let mut st = c.prepare(
            "SELECT id, name, email, imap_host, imap_port, smtp_host, smtp_port, color, signature, position, label, aliases_json, reply_to
             FROM accounts ORDER BY position, id",
        )?;
        let rows = st.query_map([], row_to_account)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn account(&self, id: i64) -> Result<Account> {
        let c = self.r();
        Ok(c.query_row(
            "SELECT id, name, email, imap_host, imap_port, smtp_host, smtp_port, color, signature, position, label, aliases_json, reply_to
             FROM accounts WHERE id = ?",
            [id],
            row_to_account,
        )?)
    }

    pub fn save_account(&self, a: &Account) -> Result<i64> {
        let c = self.c();
        let aliases_json = serde_json::to_string(&a.aliases)?;
        if a.id > 0 {
            c.execute(
                "UPDATE accounts SET name=?, email=?, imap_host=?, imap_port=?, smtp_host=?, smtp_port=?, color=?, signature=?, label=?, aliases_json=?, reply_to=? WHERE id=?",
                params![a.name, a.email, a.imap_host, a.imap_port, a.smtp_host, a.smtp_port, a.color, a.signature, a.label, aliases_json, a.reply_to, a.id],
            )?;
            Ok(a.id)
        } else {
            let pos: i64 = c.query_row("SELECT COALESCE(MAX(position), 0) + 1 FROM accounts", [], |r| r.get(0))?;
            c.execute(
                "INSERT INTO accounts (name, email, imap_host, imap_port, smtp_host, smtp_port, color, signature, position, label, aliases_json, reply_to)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![a.name, a.email, a.imap_host, a.imap_port, a.smtp_host, a.smtp_port, a.color, a.signature, pos, a.label, aliases_json, a.reply_to],
            )?;
            Ok(c.last_insert_rowid())
        }
    }

    pub fn reorder_accounts(&self, ids: &[i64]) -> Result<()> {
        let mut c = self.c();
        let tx = c.transaction()?;
        for (i, id) in ids.iter().enumerate() {
            tx.execute("UPDATE accounts SET position = ? WHERE id = ?", params![i as i64, id])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn delete_account(&self, id: i64) -> Result<()> {
        let mut c = self.c();
        let tx = c.transaction()?;
        tx.execute(
            "DELETE FROM messages_fts WHERE rowid IN (SELECT id FROM messages WHERE account_id = ?)",
            [id],
        )?;
        tx.execute("DELETE FROM rule_seen WHERE account_id = ?", [id])?;
        tx.execute("DELETE FROM rule_log WHERE account_id = ?", [id])?;
        // Remove rules scoped solely to this account and their reply history.
        let rule_ids: Vec<i64> = tx
            .prepare("SELECT id, def FROM rules")?
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
            .filter_map(|r| r.ok())
            .filter_map(|(rid, def)| {
                serde_json::from_str::<Rule>(&def).ok().filter(|r| r.account_id == Some(id)).map(|_| rid)
            })
            .collect();
        for rid in rule_ids {
            tx.execute("DELETE FROM rule_replies WHERE rule_id = ?", [rid])?;
            tx.execute("DELETE FROM rules WHERE id = ?", [rid])?;
        }
        tx.execute("DELETE FROM accounts WHERE id = ?", [id])?;
        tx.commit()?;
        Ok(())
    }

    // ---------- folders ----------

    pub fn upsert_folder(&self, account_id: i64, path: &str, name: &str, role: Option<&str>) -> Result<i64> {
        let c = self.c();
        c.execute(
            "INSERT INTO folders (account_id, path, name, role) VALUES (?, ?, ?, ?)
             ON CONFLICT(account_id, path) DO UPDATE SET name = excluded.name, role = excluded.role",
            params![account_id, path, name, role],
        )?;
        Ok(c.query_row(
            "SELECT id FROM folders WHERE account_id = ? AND path = ?",
            params![account_id, path],
            |r| r.get(0),
        )?)
    }

    pub fn rename_folder_local(&self, folder_id: i64, new_path: &str, new_name: &str) -> Result<()> {
        let mut c = self.c();
        let tx = c.transaction()?;
        let (account_id, old_path): (i64, String) = tx.query_row(
            "SELECT account_id, path FROM folders WHERE id = ?",
            [folder_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        tx.execute(
            "UPDATE folders SET path = ?, name = ? WHERE id = ?",
            params![new_path, new_name, folder_id],
        )?;
        // Also update nested child paths sharing the same delimiter prefix.
        for sep in ['/', '.'] {
            let old_prefix = format!("{old_path}{sep}");
            let children: Vec<(i64, String)> = tx
                .prepare("SELECT id, path FROM folders WHERE account_id = ? AND path LIKE ?")?
                .query_map(params![account_id, format!("{old_prefix}%")], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            for (cid, cpath) in children {
                if let Some(rest) = cpath.strip_prefix(&old_prefix) {
                    tx.execute("UPDATE folders SET path = ? WHERE id = ?", params![format!("{new_path}{sep}{rest}"), cid])?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn delete_folder_local(&self, folder_id: i64) -> Result<()> {
        let mut c = self.c();
        let tx = c.transaction()?;
        tx.execute("DELETE FROM messages_fts WHERE rowid IN (SELECT id FROM messages WHERE folder_id = ?)", [folder_id])?;
        tx.execute("DELETE FROM folders WHERE id = ?", [folder_id])?;
        tx.commit()?;
        Ok(())
    }

    pub fn clear_folder_messages(&self, folder_id: i64) -> Result<()> {
        let mut c = self.c();
        let tx = c.transaction()?;
        tx.execute("DELETE FROM messages_fts WHERE rowid IN (SELECT id FROM messages WHERE folder_id = ?)", [folder_id])?;
        tx.execute("DELETE FROM messages WHERE folder_id = ?", [folder_id])?;
        tx.execute(
            "UPDATE folders SET msg_count = 0, unseen_count = 0 WHERE id = ?",
            [folder_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn remove_folders_except(&self, account_id: i64, keep: &[i64]) -> Result<()> {
        let mut c = self.c();
        let tx = c.transaction()?;
        let existing: Vec<i64> = tx
            .prepare("SELECT id FROM folders WHERE account_id = ?")?
            .query_map([account_id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for id in existing.into_iter().filter(|id| !keep.contains(id)) {
            tx.execute("DELETE FROM messages_fts WHERE rowid IN (SELECT id FROM messages WHERE folder_id = ?)", [id])?;
            tx.execute("DELETE FROM folders WHERE id = ?", [id])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Folders in display order: accounts in their order, then each account's
    /// folders by user position, falling back to a sensible default.
    pub fn folders(&self) -> Result<Vec<Folder>> {
        let c = self.r();
        let mut st = c.prepare(
            "SELECT f.id, f.account_id, f.path, f.name, f.role,
                    (SELECT COUNT(*) FROM messages m WHERE m.folder_id = f.id AND m.seen = 0),
                    f.position
             FROM folders f JOIN accounts a ON a.id = f.account_id
             ORDER BY a.position, a.id,
                      f.position IS NULL, f.position,
                      CASE f.role WHEN 'inbox' THEN 0 WHEN 'drafts' THEN 1 WHEN 'sent' THEN 2
                                  WHEN 'archive' THEN 3 WHEN 'junk' THEN 4 WHEN 'trash' THEN 5
                                  WHEN 'all' THEN 7 ELSE 6 END,
                      f.name COLLATE NOCASE",
        )?;
        let rows = st
            .query_map([], |r| {
                Ok(Folder {
                    id: r.get(0)?,
                    account_id: r.get(1)?,
                    path: r.get(2)?,
                    name: r.get(3)?,
                    role: r.get(4)?,
                    unread: r.get(5)?,
                    position: r.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Saves the order of one account's folders. Folders not listed keep their place after these.
    pub fn reorder_folders(&self, account_id: i64, ids: &[i64]) -> Result<()> {
        let mut c = self.c();
        let tx = c.transaction()?;
        for (i, id) in ids.iter().enumerate() {
            tx.execute(
                "UPDATE folders SET position = ? WHERE id = ? AND account_id = ?",
                params![i as i64, id, account_id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// (id, path, role) of one account's folders.
    pub fn account_folders(&self, account_id: i64) -> Result<Vec<(i64, String, Option<String>)>> {
        let c = self.r();
        let rows = c
            .prepare("SELECT id, path, role FROM folders WHERE account_id = ?")?
            .query_map([account_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn folder_by_path(&self, account_id: i64, path: &str) -> Result<Option<(i64, Option<String>)>> {
        let c = self.r();
        Ok(c.query_row(
            "SELECT id, role FROM folders WHERE account_id = ? AND path = ?",
            params![account_id, path],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
    }

    pub fn folder(&self, id: i64) -> Result<(i64, String, Option<String>)> {
        let c = self.r();
        Ok(c.query_row("SELECT account_id, path, role FROM folders WHERE id = ?", [id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?)
    }

    pub fn folder_by_role(&self, account_id: i64, role: &str) -> Result<Option<(i64, String)>> {
        let c = self.r();
        Ok(c.query_row(
            "SELECT id, path FROM folders WHERE account_id = ? AND role = ?",
            params![account_id, role],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
    }

    pub fn folder_sync_state(&self, folder_id: i64) -> Result<FolderSyncState> {
        let c = self.r();
        Ok(c.query_row(
            "SELECT uidvalidity, synced, uidnext, msg_count, unseen_count, highest_modseq, last_full_sync FROM folders WHERE id = ?",
            [folder_id],
            |r| {
                let uv: i64 = r.get(0)?;
                let uidnext: i64 = r.get(2)?;
                let modseq: i64 = r.get(5)?;
                Ok(FolderSyncState {
                    uidvalidity: uv.max(0) as u32,
                    synced: r.get(1)?,
                    uidnext: uidnext.max(0) as u32,
                    msg_count: r.get(3)?,
                    unseen_count: r.get(4)?,
                    highest_modseq: modseq.max(0) as u64,
                    last_full_sync: r.get(6)?,
                })
            },
        )?)
    }

    pub fn update_folder_sync_state(&self, folder_id: i64, st: &FolderSyncState) -> Result<()> {
        self.c().execute(
            "UPDATE folders SET uidvalidity = ?, synced = ?, uidnext = ?, msg_count = ?, unseen_count = ?, highest_modseq = ?, last_full_sync = ? WHERE id = ?",
            params![
                st.uidvalidity as i64,
                st.synced,
                st.uidnext as i64,
                st.msg_count,
                st.unseen_count,
                st.highest_modseq as i64,
                st.last_full_sync,
                folder_id,
            ],
        )?;
        Ok(())
    }

    /// Invalidates cached EXISTS/UNSEEN counts after a local mutation changes the server mailbox.
    pub fn invalidate_folder_counts(&self, folder_id: i64) -> Result<()> {
        self.c().execute(
            "UPDATE folders SET msg_count = -1, unseen_count = -1 WHERE id = ?",
            [folder_id],
        )?;
        Ok(())
    }

    #[cfg(test)]
    pub fn folder_synced(&self, folder_id: i64) -> Result<bool> {
        let c = self.r();
        let v: bool = c.query_row("SELECT synced FROM folders WHERE id = ?", [folder_id], |r| r.get(0))?;
        Ok(v)
    }

    #[cfg(test)]
    pub fn mark_folder_synced(&self, folder_id: i64) -> Result<()> {
        self.c().execute("UPDATE folders SET synced = 1 WHERE id = ?", [folder_id])?;
        Ok(())
    }

    pub fn reset_folder(&self, folder_id: i64, uidvalidity: u32) -> Result<()> {
        let mut c = self.c();
        let tx = c.transaction()?;
        tx.execute("DELETE FROM messages_fts WHERE rowid IN (SELECT id FROM messages WHERE folder_id = ?)", [folder_id])?;
        tx.execute("DELETE FROM messages WHERE folder_id = ?", [folder_id])?;
        tx.execute(
            "UPDATE folders SET uidvalidity = ?, synced = 0, uidnext = 0, msg_count = -1, unseen_count = -1, highest_modseq = 0, last_full_sync = 0 WHERE id = ?",
            params![uidvalidity as i64, folder_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    // ---------- pending IMAP operations (durable offline/race-safe queue) ----------

    pub fn enqueue_pending_op(&self, account_id: i64, folder_id: i64, payload: &PendingOpPayload, uids: &[u32], now: i64) -> Result<i64> {
        let kind = match payload {
            PendingOpPayload::Store { .. } => "store",
            PendingOpPayload::MarkAllRead => "mark_all_read",
            PendingOpPayload::MoveRole { .. } => "move_role",
            PendingOpPayload::MoveFolder { .. } => "move_folder",
            PendingOpPayload::Delete => "delete",
            PendingOpPayload::EmptyFolder => "empty_folder",
        };
        let c = self.c();
        c.execute(
            "INSERT INTO pending_ops (account_id, folder_id, kind, payload, uids_json, created_at) VALUES (?, ?, ?, ?, ?, ?)",
            params![account_id, folder_id, kind, serde_json::to_string(payload)?, serde_json::to_string(uids)?, now],
        )?;
        Ok(c.last_insert_rowid())
    }

    pub fn due_pending_ops(&self, account_id: i64, folder_id: Option<i64>, now: i64) -> Result<Vec<PendingOp>> {
        let c = self.r();
        let (sql, args): (&str, Vec<rusqlite::types::Value>) = match folder_id {
            Some(fid) => (
                "SELECT id, account_id, folder_id, payload, uids_json, attempts FROM pending_ops
                 WHERE account_id = ?1 AND folder_id = ?2 AND next_retry <= ?3 ORDER BY id ASC",
                vec![account_id.into(), fid.into(), now.into()],
            ),
            None => (
                "SELECT id, account_id, folder_id, payload, uids_json, attempts FROM pending_ops
                 WHERE account_id = ?1 AND next_retry <= ?2 ORDER BY id ASC",
                vec![account_id.into(), now.into()],
            ),
        };
        let mut st = c.prepare(sql)?;
        let rows = st
            .query_map(rusqlite::params_from_iter(args), |r| {
                let payload_s: String = r.get(3)?;
                let uids_s: String = r.get(4)?;
                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, payload_s, uids_s, r.get::<_, i64>(5)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut out = Vec::with_capacity(rows.len());
        for (id, _acc, fid, payload_s, uids_s, attempts) in rows {
            if let (Ok(payload), Ok(uids)) = (serde_json::from_str(&payload_s), serde_json::from_str(&uids_s)) {
                out.push(PendingOp { id, folder_id: fid, payload, uids, attempts });
            }
        }
        Ok(out)
    }

    pub fn delete_pending_op(&self, id: i64) -> Result<()> {
        self.c().execute("DELETE FROM pending_ops WHERE id = ?", [id])?;
        Ok(())
    }

    pub fn fail_pending_op(&self, id: i64, next_retry: i64) -> Result<()> {
        self.c().execute(
            "UPDATE pending_ops SET attempts = attempts + 1, next_retry = ? WHERE id = ?",
            params![next_retry, id],
        )?;
        Ok(())
    }

    pub fn has_pending_ops_for_folder(&self, folder_id: i64) -> Result<bool> {
        let c = self.r();
        let n: i64 = c.query_row("SELECT COUNT(*) FROM pending_ops WHERE folder_id = ?", [folder_id], |r| r.get(0))?;
        Ok(n > 0)
    }

    /// UIDs in `folder_id` that have un-flushed local mutations (flags, moves, deletes).
    /// Also returns `true` in the second tuple element if a folder-wide op (MarkAllRead) is pending.
    pub fn pending_uids_for_folder(&self, folder_id: i64) -> Result<(std::collections::HashSet<u32>, bool)> {
        let c = self.r();
        let mut st = c.prepare("SELECT kind, uids_json FROM pending_ops WHERE folder_id = ?")?;
        let rows: Vec<(String, String)> = st
            .query_map([folder_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut uids = std::collections::HashSet::new();
        let mut mark_all_read = false;
        for (kind, uids_s) in rows {
            if kind == "mark_all_read" {
                mark_all_read = true;
            } else if let Ok(list) = serde_json::from_str::<Vec<u32>>(&uids_s) {
                uids.extend(list);
            }
        }
        Ok((uids, mark_all_read))
    }

    // ---------- messages ----------

    /// Bounds of the window of recent mail kept in step with the server (search hits excluded).
    pub fn uid_range(&self, folder_id: i64) -> Result<(u32, u32)> {
        let c = self.r();
        let (min, max): (Option<i64>, Option<i64>) = c.query_row(
            "SELECT MIN(uid), MAX(uid) FROM messages WHERE folder_id = ? AND sparse = 0",
            [folder_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok((min.unwrap_or(0) as u32, max.unwrap_or(0) as u32))
    }

    /// Search hits stored below the sync window: (uid) for a folder, below `below`.
    pub fn sparse_uids_below(&self, folder_id: i64, below: u32) -> Result<Vec<u32>> {
        let c = self.r();
        let rows = c
            .prepare("SELECT uid FROM messages WHERE folder_id = ? AND sparse = 1 AND uid < ?")?
            .query_map(params![folder_id, below], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// Of these uids, the ones not stored yet (loaded in one query instead of N+1).
    pub fn missing_uids(&self, folder_id: i64, uids: &[u32]) -> Result<Vec<u32>> {
        if uids.is_empty() {
            return Ok(vec![]);
        }
        let c = self.r();
        let min_u = *uids.iter().min().unwrap();
        let max_u = *uids.iter().max().unwrap();
        let existing: std::collections::HashSet<u32> = c
            .prepare("SELECT uid FROM messages WHERE folder_id = ? AND uid >= ? AND uid <= ?")?
            .query_map(params![folder_id, min_u, max_u], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(uids.iter().copied().filter(|u| !existing.contains(u)).collect())
    }

    pub fn ids_for_uids(&self, folder_id: i64, uids: &[u32]) -> Result<Vec<i64>> {
        let c = self.r();
        let mut st = c.prepare("SELECT id FROM messages WHERE folder_id = ? AND uid = ?")?;
        let mut out = Vec::new();
        for u in uids {
            if let Some(id) = st.query_row(params![folder_id, u], |r| r.get(0)).optional()? {
                out.push(id);
            }
        }
        Ok(out)
    }

    /// Stores headers. `sparse` marks search hits that lie outside the sync window.
    /// Returns the ids of the rows that are new.
    pub fn insert_messages(&self, account_id: i64, folder_id: i64, msgs: &[NewMessage]) -> Result<Vec<i64>> {
        self.insert_messages_as(account_id, folder_id, msgs, false)
    }

    pub fn insert_messages_as(&self, account_id: i64, folder_id: i64, msgs: &[NewMessage], sparse: bool) -> Result<Vec<i64>> {
        let mut added = Vec::new();
        let mut c = self.c();
        let tx = c.transaction()?;
        {
            let mut ins = tx.prepare(
                "INSERT OR IGNORE INTO messages (account_id, folder_id, uid, message_id, in_reply_to, refs, subject,
                    from_name, from_addr, to_json, cc_json, reply_to_json, date, seen, flagged, has_attachments, size, sparse,
                    list_id, auto)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )?;
            // Sync reaching a stored search hit takes it into the window.
            let mut unsparse = tx.prepare("UPDATE messages SET sparse = 0 WHERE folder_id = ? AND uid = ? AND sparse = 1")?;
            let mut fts = tx.prepare("INSERT INTO messages_fts (rowid, subject, sender, body) VALUES (?, ?, ?, '')")?;
            for m in msgs {
                let n = ins.execute(params![
                    account_id,
                    folder_id,
                    m.uid,
                    m.message_id,
                    m.in_reply_to,
                    m.refs,
                    m.subject,
                    m.from_name,
                    m.from_addr,
                    serde_json::to_string(&m.to)?,
                    serde_json::to_string(&m.cc)?,
                    serde_json::to_string(&m.reply_to)?,
                    m.date,
                    m.seen,
                    m.flagged,
                    m.has_attachments,
                    m.size,
                    sparse,
                    m.list_id,
                    m.auto,
                ])?;
                if n > 0 {
                    let id = tx.last_insert_rowid();
                    added.push(id);
                    fts.execute(params![id, m.subject, format!("{} {}", m.from_name, m.from_addr)])?;
                    assign_thread(&tx, account_id, id, m.message_id.as_deref(), m.in_reply_to.as_deref(), m.refs.as_deref())?;
                } else if !sparse {
                    unsparse.execute(params![folder_id, m.uid])?;
                }
            }
        }
        tx.commit()?;
        Ok(added)
    }

    /// Single-pass reconciliation: applies server flags for every uid >= `from_uid`, deletes
    /// rows the server no longer has, and returns `(changed, missing_server_uids)`.
    /// UIDs in `skip_uids` (which have un-flushed local ops) are never reverted or resurrected.
    pub fn reconcile_flags_and_missing(
        &self,
        folder_id: i64,
        from_uid: u32,
        flags: &[(u32, bool, bool)],
        skip_uids: &std::collections::HashSet<u32>,
        skip_seen_revert: bool,
    ) -> Result<(bool, Vec<u32>)> {
        let mut c = self.c();
        let tx = c.transaction()?;
        let mut changed = false;
        let mut missing = Vec::new();
        {
            let local: Vec<(i64, u32, bool, bool)> = tx
                .prepare("SELECT id, uid, seen, flagged FROM messages WHERE folder_id = ? AND uid >= ?")?
                .query_map(params![folder_id, from_uid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                .collect::<rusqlite::Result<_>>()?;
            let mut local_uids = std::collections::HashSet::with_capacity(local.len());
            let server: std::collections::HashMap<u32, (bool, bool)> =
                flags.iter().map(|(u, s, f)| (*u, (*s, *f))).collect();
            let mut del_fts = tx.prepare("DELETE FROM messages_fts WHERE rowid = ?")?;
            let mut del_msg = tx.prepare("DELETE FROM messages WHERE id = ?")?;
            let mut upd_msg = tx.prepare("UPDATE messages SET seen = ?, flagged = ? WHERE id = ?")?;
            for (id, uid, seen, flagged) in local {
                local_uids.insert(uid);
                if skip_uids.contains(&uid) {
                    continue;
                }
                match server.get(&uid) {
                    None => {
                        del_fts.execute([id])?;
                        del_msg.execute([id])?;
                        changed = true;
                    }
                    Some(&(s, f)) => {
                        let next_seen = if skip_seen_revert && seen { true } else { s };
                        if next_seen != seen || f != flagged {
                            upd_msg.execute(params![next_seen, f, id])?;
                            changed = true;
                        }
                    }
                }
            }
            for &(u, _, _) in flags {
                if u >= from_uid && !local_uids.contains(&u) && !skip_uids.contains(&u) {
                    missing.push(u);
                }
            }
        }
        tx.commit()?;
        Ok((changed, missing))
    }

    /// Applies CONDSTORE CHANGEDSINCE flag updates without deleting unmentioned rows.
    pub fn apply_changed_flags(
        &self,
        folder_id: i64,
        flags: &[(u32, bool, bool)],
        skip_uids: &std::collections::HashSet<u32>,
        skip_seen_revert: bool,
    ) -> Result<bool> {
        if flags.is_empty() {
            return Ok(false);
        }
        let mut c = self.c();
        let tx = c.transaction()?;
        let mut changed = false;
        {
            let mut sel = tx.prepare("SELECT id, seen, flagged FROM messages WHERE folder_id = ? AND uid = ?")?;
            let mut upd = tx.prepare("UPDATE messages SET seen = ?, flagged = ? WHERE id = ?")?;
            for &(uid, s, f) in flags {
                if skip_uids.contains(&uid) {
                    continue;
                }
                let row: Option<(i64, bool, bool)> = sel
                    .query_row(params![folder_id, uid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                    .optional()?;
                if let Some((id, seen, flagged)) = row {
                    let next_seen = if skip_seen_revert && seen { true } else { s };
                    if next_seen != seen || f != flagged {
                        upd.execute(params![next_seen, f, id])?;
                        changed = true;
                    }
                }
            }
        }
        tx.commit()?;
        Ok(changed)
    }

    /// Reconciles deletions and missing UIDs using a compact UID set (e.g., from UID SEARCH).
    pub fn reconcile_existing_uids(
        &self,
        folder_id: i64,
        from_uid: u32,
        server_uids: &std::collections::HashSet<u32>,
        skip_uids: &std::collections::HashSet<u32>,
    ) -> Result<(bool, Vec<u32>)> {
        let mut c = self.c();
        let tx = c.transaction()?;
        let mut changed = false;
        let mut missing = Vec::new();
        {
            let local: Vec<(i64, u32)> = tx
                .prepare("SELECT id, uid FROM messages WHERE folder_id = ? AND uid >= ?")?
                .query_map(params![folder_id, from_uid], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            let mut local_uids = std::collections::HashSet::with_capacity(local.len());
            let mut del_fts = tx.prepare("DELETE FROM messages_fts WHERE rowid = ?")?;
            let mut del_msg = tx.prepare("DELETE FROM messages WHERE id = ?")?;
            for (id, uid) in local {
                local_uids.insert(uid);
                if skip_uids.contains(&uid) {
                    continue;
                }
                if !server_uids.contains(&uid) {
                    del_fts.execute([id])?;
                    del_msg.execute([id])?;
                    changed = true;
                }
            }
            for &u in server_uids {
                if u >= from_uid && !local_uids.contains(&u) && !skip_uids.contains(&u) {
                    missing.push(u);
                }
            }
        }
        tx.commit()?;
        Ok((changed, missing))
    }

    /// Applies server flags to exactly these uids; ones the server didn't return are gone.
    pub fn reconcile_uids(
        &self,
        folder_id: i64,
        uids: &[u32],
        flags: &[(u32, bool, bool)],
        skip_uids: &std::collections::HashSet<u32>,
    ) -> Result<bool> {
        let mut c = self.c();
        let tx = c.transaction()?;
        let server: std::collections::HashMap<u32, (bool, bool)> = flags.iter().map(|(u, s, f)| (*u, (*s, *f))).collect();
        let mut changed = false;
        for uid in uids {
            if skip_uids.contains(uid) {
                continue;
            }
            let row: Option<(i64, bool, bool)> = tx
                .query_row("SELECT id, seen, flagged FROM messages WHERE folder_id = ? AND uid = ?", params![folder_id, uid], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })
                .optional()?;
            let Some((id, seen, flagged)) = row else { continue };
            match server.get(uid) {
                None => {
                    tx.execute("DELETE FROM messages_fts WHERE rowid = ?", [id])?;
                    tx.execute("DELETE FROM messages WHERE id = ?", [id])?;
                    changed = true;
                }
                Some(&(s, f)) if s != seen || f != flagged => {
                    tx.execute("UPDATE messages SET seen = ?, flagged = ? WHERE id = ?", params![s, f, id])?;
                    changed = true;
                }
                _ => {}
            }
        }
        tx.commit()?;
        Ok(changed)
    }

    /// Recent messages missing bodies in `folder_id`, as `(uid, size)` pairs.
    /// Skips messages whose headers indicate attachments or that exceed `max_size`.
    pub fn missing_bodies_with_size(&self, folder_id: i64, limit: usize, max_size: u32) -> Result<Vec<(u32, u32)>> {
        let c = self.r();
        let mut st = c.prepare(
            "SELECT uid, size FROM messages WHERE folder_id = ? AND has_body = 0 AND has_attachments = 0 AND size <= ? ORDER BY date DESC LIMIT ?",
        )?;
        let rows = st
            .query_map(params![folder_id, max_size, limit as i64], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<(u32, u32)>>>()?;
        Ok(rows)
    }

    pub fn store_body(&self, folder_id: i64, uid: u32, raw: &[u8], info: &BodyInfo) -> Result<()> {
        let mut c = self.c();
        let tx = c.transaction()?;
        let id: Option<i64> = tx
            .query_row("SELECT id FROM messages WHERE folder_id = ? AND uid = ?", params![folder_id, uid], |r| r.get(0))
            .optional()?;
        if let Some(id) = id {
            tx.execute("INSERT OR REPLACE INTO bodies (message_id, raw) VALUES (?, ?)", params![id, raw])?;
            tx.execute(
                "UPDATE messages SET has_body = 1, snippet = ?, has_attachments = ? WHERE id = ?",
                params![info.snippet, info.has_attachments, id],
            )?;
            tx.execute("UPDATE messages_fts SET body = ? WHERE rowid = ?", params![info.search_text, id])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Stores a whole batch of prefetched bodies in a single SQLite transaction.
    /// Messages that turn out to have attachments are NOT stored in `bodies` (only flagged
    /// `has_attachments = 1` so future prefetch passes skip them).
    pub fn store_bodies_batch(&self, folder_id: i64, items: &[(u32, Vec<u8>, BodyInfo)]) -> Result<usize> {
        if items.is_empty() {
            return Ok(0);
        }
        let mut c = self.c();
        let tx = c.transaction()?;
        let mut stored = 0;
        {
            let mut find_id = tx.prepare("SELECT id FROM messages WHERE folder_id = ? AND uid = ?")?;
            let mut ins_body = tx.prepare("INSERT OR REPLACE INTO bodies (message_id, raw) VALUES (?, ?)")?;
            let mut upd_msg = tx.prepare("UPDATE messages SET has_body = 1, snippet = ?, has_attachments = 0 WHERE id = ?")?;
            let mut mark_att = tx.prepare("UPDATE messages SET has_attachments = 1 WHERE id = ?")?;
            let mut upd_fts = tx.prepare("UPDATE messages_fts SET body = ? WHERE rowid = ?")?;
            for (uid, raw, info) in items {
                let id: Option<i64> = find_id.query_row(params![folder_id, uid], |r| r.get(0)).optional()?;
                if let Some(id) = id {
                    if info.has_attachments {
                        mark_att.execute(params![id])?;
                        continue;
                    }
                    ins_body.execute(params![id, raw.as_slice()])?;
                    upd_msg.execute(params![info.snippet, id])?;
                    upd_fts.execute(params![info.search_text, id])?;
                    stored += 1;
                }
            }
        }
        tx.commit()?;
        Ok(stored)
    }

    /// Preserves bodies of messages about to be optimistically removed by Move/Archive/Trash
    /// so the destination copy can hydrate them locally without re-downloading over IMAP.
    pub fn stash_moved_bodies(&self, ids: &[i64], now: i64) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let mut c = self.c();
        let tx = c.transaction()?;
        {
            let mut sel = tx.prepare(
                "SELECT m.account_id, m.message_id, b.raw, m.snippet, COALESCE(f.body, ''), m.has_attachments
                 FROM messages m
                 JOIN bodies b ON b.message_id = m.id
                 LEFT JOIN messages_fts f ON f.rowid = m.id
                 WHERE m.id = ? AND m.message_id IS NOT NULL AND m.has_body = 1",
            )?;
            let mut ins = tx.prepare(
                "INSERT OR REPLACE INTO moved_bodies (account_id, message_id, raw, snippet, search_text, has_attachments, created_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
            )?;
            for &id in ids {
                let row: Option<(i64, String, Vec<u8>, String, String, bool)> = sel
                    .query_row([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))
                    .optional()?;
                if let Some((acc, mid, raw, snippet, search_text, has_att)) = row {
                    ins.execute(params![acc, mid, raw, snippet, search_text, has_att, now])?;
                }
            }
            // Keep the stash small and short-lived.
            tx.execute("DELETE FROM moved_bodies WHERE created_at < ?", [now - 3600])?;
            tx.execute(
                "DELETE FROM moved_bodies WHERE rowid NOT IN (SELECT rowid FROM moved_bodies ORDER BY created_at DESC LIMIT 200)",
                [],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Hydrates messages missing bodies in `folder_id` from either `moved_bodies` or another
    /// folder in the same account that already holds the same `Message-ID` (e.g., Gmail labels).
    pub fn copy_existing_bodies_for_folder(&self, account_id: i64, folder_id: i64) -> Result<usize> {
        let mut c = self.c();
        let tx = c.transaction()?;
        let mut hydrated = 0;
        {
            let candidates: Vec<(i64, String)> = tx
                .prepare("SELECT id, message_id FROM messages WHERE folder_id = ? AND has_body = 0 AND message_id IS NOT NULL")?
                .query_map([folder_id], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            if !candidates.is_empty() {
                let mut from_stash = tx.prepare(
                    "SELECT raw, snippet, search_text, has_attachments FROM moved_bodies WHERE account_id = ? AND message_id = ?",
                )?;
                let mut del_stash = tx.prepare("DELETE FROM moved_bodies WHERE account_id = ? AND message_id = ?")?;
                let mut from_other = tx.prepare(
                    "SELECT b.raw, m.snippet, COALESCE(f.body, ''), m.has_attachments
                     FROM messages m
                     JOIN bodies b ON b.message_id = m.id
                     LEFT JOIN messages_fts f ON f.rowid = m.id
                     WHERE m.account_id = ? AND m.message_id = ? AND m.has_body = 1 AND m.id != ?
                     LIMIT 1",
                )?;
                let mut ins_body = tx.prepare("INSERT OR REPLACE INTO bodies (message_id, raw) VALUES (?, ?)")?;
                let mut upd_msg = tx.prepare("UPDATE messages SET has_body = 1, snippet = ?, has_attachments = ? WHERE id = ?")?;
                let mut upd_fts = tx.prepare("UPDATE messages_fts SET body = ? WHERE rowid = ?")?;

                for (id, mid) in candidates {
                    let stashed: Option<(Vec<u8>, String, String, bool)> = from_stash
                        .query_row(params![account_id, &mid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
                        .optional()?;
                    let found = match stashed {
                        Some(v) => {
                            del_stash.execute(params![account_id, &mid])?;
                            Some(v)
                        }
                        None => from_other
                            .query_row(params![account_id, &mid, id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
                            .optional()?,
                    };
                    if let Some((raw, snippet, search_text, has_att)) = found {
                        ins_body.execute(params![id, raw])?;
                        upd_msg.execute(params![snippet, has_att, id])?;
                        upd_fts.execute(params![search_text, id])?;
                        hydrated += 1;
                    }
                }
            }
        }
        tx.commit()?;
        Ok(hydrated)
    }

    /// Evicts oldest cached raw bodies for `account_id` when total stored body size exceeds `max_bytes`.
    /// Preserves snippets and FTS body text; prefers keeping recent Inbox bodies.
    pub fn prune_old_bodies(&self, account_id: i64, max_bytes: i64) -> Result<usize> {
        let total: i64 = self.r().query_row(
            "SELECT COALESCE(SUM(LENGTH(b.raw)), 0) FROM bodies b JOIN messages m ON m.id = b.message_id WHERE m.account_id = ?",
            [account_id],
            |r| r.get(0),
        )?;
        if total <= max_bytes {
            return Ok(0);
        }
        let target = (max_bytes * 8) / 10;
        let mut to_drop = total - target;
        let mut c = self.c();
        let tx = c.transaction()?;
        let mut evicted = 0;
        {
            let candidates: Vec<(i64, i64)> = tx
                .prepare(
                    "SELECT m.id, LENGTH(b.raw)
                     FROM bodies b
                     JOIN messages m ON m.id = b.message_id
                     JOIN folders f ON f.id = m.folder_id
                     WHERE m.account_id = ?
                     ORDER BY (f.role IS 'inbox') ASC, m.date ASC, m.id ASC",
                )?
                .query_map([account_id], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            let mut del_body = tx.prepare("DELETE FROM bodies WHERE message_id = ?")?;
            let mut clear_flag = tx.prepare("UPDATE messages SET has_body = 0 WHERE id = ?")?;
            for (id, sz) in candidates {
                if to_drop <= 0 {
                    break;
                }
                del_body.execute([id])?;
                clear_flag.execute([id])?;
                to_drop -= sz;
                evicted += 1;
            }
        }
        tx.commit()?;
        Ok(evicted)
    }

    pub fn body(&self, id: i64) -> Result<Option<Vec<u8>>> {
        let c = self.r();
        Ok(c.query_row("SELECT raw FROM bodies WHERE message_id = ?", [id], |r| r.get(0)).optional()?)
    }

    /// (account_id, folder_id, folder_path, uid)
    pub fn locate(&self, id: i64) -> Result<(i64, i64, String, u32)> {
        let c = self.r();
        Ok(c.query_row(
            "SELECT m.account_id, m.folder_id, f.path, m.uid FROM messages m JOIN folders f ON f.id = m.folder_id WHERE m.id = ?",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?)
    }

    /// One row per conversation: its newest message in the view, standing for all of its
    /// messages in the view (`ids`). Conversations are ordered by their freshest message.
    pub fn list_threads(&self, view: &View, offset: i64, limit: i64) -> Result<Vec<MessageSummary>> {
        let c = self.r();
        let scope = scope_sql(view)?;
        let sql = format!(
            "WITH scoped AS (
               SELECT m.*, {SORT_DATE} AS sort_date FROM messages m WHERE {scope}
             ), ranked AS (
               SELECT scoped.*,
                      ROW_NUMBER() OVER (PARTITION BY account_id, COALESCE(thread_id, 'm' || id) ORDER BY date DESC, id DESC) AS rn,
                      MIN(seen) OVER t AS t_seen,
                      MAX(flagged) OVER t AS t_flagged,
                      MAX(has_attachments) OVER t AS t_att,
                      MAX(sort_date) OVER t AS t_sort,
                      GROUP_CONCAT(id) OVER t AS t_ids
               FROM scoped
               WINDOW t AS (PARTITION BY account_id, COALESCE(thread_id, 'm' || id))
             )
             SELECT {SUMMARY_COLS}, m.t_seen, m.t_flagged, m.t_att, m.t_ids,
                    (SELECT COUNT(DISTINCT COALESCE(x.message_id, x.id)) FROM messages x
                     WHERE x.account_id = m.account_id AND x.thread_id = m.thread_id
                       AND x.folder_id NOT IN (SELECT id FROM folders WHERE role IN ('trash', 'junk')))
             FROM ranked m WHERE m.rn = 1
             ORDER BY m.t_sort DESC, m.id DESC LIMIT ?1 OFFSET ?2"
        );
        let mut st = c.prepare(&sql)?;
        let rows = st
            .query_map(params![limit, offset], |r| {
                let mut s = row_to_summary(r)?;
                s.seen = r.get(SUMMARY_LEN)?;
                s.flagged = r.get(SUMMARY_LEN + 1)?;
                s.has_attachments = r.get(SUMMARY_LEN + 2)?;
                let ids: String = r.get(SUMMARY_LEN + 3)?;
                s.ids = ids.split(',').filter_map(|i| i.parse().ok()).collect();
                let total: i64 = r.get(SUMMARY_LEN + 4)?;
                s.count = total.max(s.ids.len() as i64).max(1);
                Ok(s)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Every message of a message's conversation in its account, oldest first. Copies of one
    /// message in several folders (Gmail's All Mail) appear once. Trash and Junk are left
    /// out unless the message itself is there.
    pub fn thread(&self, id: i64) -> Result<Vec<MessageSummary>> {
        let c = self.r();
        let (account_id, thread_id, role): (i64, Option<String>, Option<String>) = c.query_row(
            "SELECT m.account_id, m.thread_id, f.role FROM messages m JOIN folders f ON f.id = m.folder_id WHERE m.id = ?",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let sql = format!("SELECT {SUMMARY_COLS}, m.message_id, f.role FROM messages m JOIN folders f ON f.id = m.folder_id WHERE m.id = ?");
        let Some(thread_id) = thread_id else {
            return Ok(vec![c.query_row(&sql, [id], row_to_summary)?]);
        };
        let hide_bin = !matches!(role.as_deref(), Some("trash") | Some("junk"));
        let sql = format!(
            "SELECT {SUMMARY_COLS}, m.message_id, f.role FROM messages m JOIN folders f ON f.id = m.folder_id
             WHERE m.account_id = ?1 AND m.thread_id = ?2 {}
             ORDER BY m.date, m.id",
            if hide_bin { "AND (f.role IS NULL OR f.role NOT IN ('trash', 'junk'))" } else { "" }
        );
        let rows: Vec<(MessageSummary, Option<String>, Option<String>)> = c
            .prepare(&sql)?
            .query_map(params![account_id, thread_id], |r| Ok((row_to_summary(r)?, r.get(SUMMARY_LEN)?, r.get(SUMMARY_LEN + 1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        // Keep one copy per Message-ID: the opened message, else one outside All Mail.
        let mut out: Vec<MessageSummary> = Vec::new();
        let mut index: std::collections::HashMap<String, (usize, bool)> = std::collections::HashMap::new();
        for (s, mid, role) in rows {
            let preferred = s.id == id || role.as_deref() != Some("all");
            match mid {
                Some(mid) => match index.get(&mid).copied() {
                    Some((i, had_pref)) => {
                        if preferred && (!had_pref || s.id == id) {
                            out[i] = s;
                            index.insert(mid, (i, true));
                        }
                    }
                    None => {
                        index.insert(mid, (out.len(), preferred));
                        out.push(s);
                    }
                },
                None => out.push(s),
            }
        }
        if !out.iter().any(|s| s.id == id) {
            out.push(c.query_row(&format!("SELECT {SUMMARY_COLS} FROM messages m WHERE m.id = ?"), [id], row_to_summary)?);
            out.sort_by_key(|s| (s.date, s.id));
        }
        Ok(out)
    }

    /// Counts for the Flagged and Unread smart views (deduplicated across All Mail).
    pub fn smart_counts(&self) -> Result<SmartCounts> {
        let c = self.r();
        let flagged_scope = scope_sql(&View::Smart { name: "flagged".into(), account_id: None })?;
        let unread_scope = scope_sql(&View::Smart { name: "unread".into(), account_id: None })?;
        let flagged: i64 = c.query_row(&format!("SELECT COUNT(*) FROM messages m WHERE {flagged_scope}"), [], |r| r.get(0))?;
        let unread: i64 = c.query_row(&format!("SELECT COUNT(*) FROM messages m WHERE {unread_scope}"), [], |r| r.get(0))?;
        Ok(SmartCounts { flagged, unread })
    }

    /// Returns every unread message id in a smart view, for "Mark All as Read".
    pub fn smart_unread_ids(&self, view: &View) -> Result<Vec<i64>> {
        let scope = scope_sql(view)?;
        let c = self.r();
        let mut st = c.prepare(&format!("SELECT m.id FROM messages m WHERE {scope} AND m.seen = 0"))?;
        let rows = st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn summaries(&self, ids: &[i64]) -> Result<Vec<MessageSummary>> {
        let c = self.r();
        let mut st = c.prepare(&format!("SELECT {SUMMARY_COLS} FROM messages m WHERE m.id = ?"))?;
        let mut out = Vec::new();
        for id in ids {
            if let Some(s) = st.query_row([id], row_to_summary).optional()? {
                out.push(s);
            }
        }
        Ok(out)
    }

    pub fn list(&self, view: &View, query: Option<&str>, offset: i64, limit: i64) -> Result<Vec<MessageSummary>> {
        let c = self.r();
        let scope = scope_sql(view)?;
        let fts = query.and_then(fts_query);
        let sql = match fts {
            Some(_) => format!(
                "SELECT {SUMMARY_COLS} FROM messages m JOIN messages_fts ON messages_fts.rowid = m.id
                 WHERE {scope} AND messages_fts MATCH ?1 ORDER BY m.date DESC LIMIT ?2 OFFSET ?3"
            ),
            None => format!(
                "SELECT {SUMMARY_COLS} FROM messages m WHERE {scope} AND ?1 IS NULL ORDER BY {SORT_DATE} DESC, m.id DESC LIMIT ?2 OFFSET ?3"
            ),
        };
        let mut st = c.prepare(&sql)?;
        let rows = st
            .query_map(params![fts, limit, offset], row_to_summary)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Searches the folders a scope covers, newest first.
    pub fn search(&self, query: &str, scope: &SearchScope, limit: i64) -> Result<Vec<MessageSummary>> {
        let Some(fts) = fts_query(query) else { return Ok(vec![]) };
        let filter = match scope {
            SearchScope::All => "1".to_string(),
            SearchScope::Account { account_id } => format!("m.account_id = {account_id}"),
            SearchScope::Unified { role } => scope_sql(&View::Unified { role: role.clone() })?,
            SearchScope::Folder { folder_id } => scope_sql(&View::Folder { folder_id: *folder_id })?,
        };
        let c = self.r();
        let sql = format!(
            "SELECT {SUMMARY_COLS} FROM messages m JOIN messages_fts ON messages_fts.rowid = m.id
             WHERE {filter} AND messages_fts MATCH ?1 ORDER BY m.date DESC LIMIT ?2"
        );
        let mut st = c.prepare(&sql)?;
        let rows = st.query_map(params![fts, limit], row_to_summary)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Structured search: one page of matches, newest first (or oldest first), and the total.
    pub fn query_mail(&self, q: &MailQuery) -> Result<MailQueryResult> {
        let (filter, args) = query_where(q)?;
        let c = self.r();
        let total: i64 = c.query_row(
            &format!("SELECT COUNT(*) FROM messages m WHERE {filter}"),
            rusqlite::params_from_iter(args.iter()),
            |r| r.get(0),
        )?;
        let limit = q.limit.unwrap_or(25).clamp(1, 200);
        let offset = q.offset.unwrap_or(0).max(0);
        let order = if q.oldest_first { "ASC" } else { "DESC" };
        let sql = format!(
            "SELECT {SUMMARY_COLS} FROM messages m WHERE {filter} ORDER BY m.date {order}, m.id {order} LIMIT {limit} OFFSET {offset}"
        );
        let mut st = c.prepare(&sql)?;
        let rows = st.query_map(rusqlite::params_from_iter(args.iter()), row_to_summary)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(MailQueryResult { total, rows })
    }

    /// Counts matches per group, biggest groups first (at most 50), and the total.
    /// `by`: sender, domain, recipient, month, year, folder or account.
    pub fn group_mail(&self, q: &MailQuery, by: &str) -> Result<(i64, Vec<MailGroup>)> {
        let (filter, args) = query_where(q)?;
        let (from, key, label) = match by {
            "sender" => ("messages m", "lower(m.from_addr)", "MAX(NULLIF(m.from_name, ''))"),
            "domain" => ("messages m", "lower(substr(m.from_addr, instr(m.from_addr, '@') + 1))", "NULL"),
            "recipient" => (
                "messages m, json_each(m.to_json) j",
                "lower(json_extract(j.value, '$.email'))",
                "MAX(json_extract(j.value, '$.name'))",
            ),
            "month" => ("messages m", "strftime('%Y-%m', m.date, 'unixepoch', 'localtime')", "NULL"),
            "year" => ("messages m", "strftime('%Y', m.date, 'unixepoch', 'localtime')", "NULL"),
            "folder" => (
                "messages m",
                "CAST(m.folder_id AS TEXT)",
                "(SELECT f.name || ' · ' || a.email FROM folders f JOIN accounts a ON a.id = f.account_id WHERE f.id = m.folder_id)",
            ),
            "account" => ("messages m", "CAST(m.account_id AS TEXT)", "(SELECT a.email FROM accounts a WHERE a.id = m.account_id)"),
            _ => return Err(anyhow!("Unknown grouping {by:?}: use sender, domain, recipient, month, year, folder or account")),
        };
        let c = self.r();
        let total: i64 = c.query_row(
            &format!("SELECT COUNT(*) FROM messages m WHERE {filter}"),
            rusqlite::params_from_iter(args.iter()),
            |r| r.get(0),
        )?;
        let sql = format!(
            "SELECT {key} AS k, {label}, COUNT(*), SUM(m.seen = 0), MAX(m.date)
             FROM {from} WHERE {filter} GROUP BY k ORDER BY COUNT(*) DESC, MAX(m.date) DESC LIMIT 50"
        );
        let mut st = c.prepare(&sql)?;
        let groups = st
            .query_map(rusqlite::params_from_iter(args.iter()), |r| {
                Ok(MailGroup {
                    key: r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                    label: r.get(1)?,
                    count: r.get(2)?,
                    unread: r.get(3)?,
                    latest: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok((total, groups))
    }

    pub fn summary(&self, id: i64) -> Result<SummaryDetailRow> {
        let c = self.r();
        let sql = format!(
            "SELECT {SUMMARY_COLS}, m.to_json, m.cc_json, m.reply_to_json, m.message_id, m.refs, m.in_reply_to FROM messages m WHERE m.id = ?"
        );
        Ok(c.query_row(&sql, [id], |r| {
            let s = row_to_summary(r)?;
            let n = SUMMARY_LEN;
            let to: String = r.get(n)?;
            let cc: String = r.get(n + 1)?;
            let rt: String = r.get(n + 2)?;
            Ok((
                s,
                serde_json::from_str(&to).unwrap_or_default(),
                serde_json::from_str(&cc).unwrap_or_default(),
                serde_json::from_str(&rt).unwrap_or_default(),
                r.get(n + 3)?,
                r.get(n + 4)?,
                r.get(n + 5)?,
            ))
        })?)
    }

    pub fn set_seen(&self, ids: &[i64], seen: bool) -> Result<()> {
        let mut c = self.c();
        let tx = c.transaction()?;
        for id in ids {
            tx.execute("UPDATE messages SET seen = ? WHERE id = ?", params![seen, id])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Marks every cached message in a folder as read.
    pub fn mark_folder_read(&self, folder_id: i64) -> Result<usize> {
        let c = self.c();
        Ok(c.execute("UPDATE messages SET seen = 1 WHERE folder_id = ? AND seen = 0", [folder_id])?)
    }

    pub fn set_flagged(&self, ids: &[i64], flagged: bool) -> Result<()> {
        let mut c = self.c();
        let tx = c.transaction()?;
        for id in ids {
            tx.execute("UPDATE messages SET flagged = ? WHERE id = ?", params![flagged, id])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn delete_messages(&self, ids: &[i64]) -> Result<()> {
        let mut c = self.c();
        let tx = c.transaction()?;
        for id in ids {
            tx.execute("DELETE FROM messages_fts WHERE rowid = ?", [id])?;
            tx.execute("DELETE FROM messages WHERE id = ?", [id])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn mark_quiet(&self, id: i64) -> Result<()> {
        self.c().execute("UPDATE messages SET quiet = 1 WHERE id = ?", [id])?;
        Ok(())
    }

    /// The Message-ID header of a stored message.
    pub fn message_id_of(&self, id: i64) -> Result<Option<String>> {
        let c = self.r();
        Ok(c.query_row("SELECT message_id FROM messages WHERE id = ?", [id], |r| r.get(0))?)
    }

    /// Removes local copies of a message from a folder (by Message-ID).
    pub fn delete_by_message_id(&self, folder_id: i64, message_id: &str) -> Result<usize> {
        let c = self.c();
        c.execute(
            "DELETE FROM messages_fts WHERE rowid IN (SELECT id FROM messages WHERE folder_id = ? AND message_id = ?)",
            params![folder_id, message_id],
        )?;
        Ok(c.execute("DELETE FROM messages WHERE folder_id = ? AND message_id = ?", params![folder_id, message_id])?)
    }

    /// The newest local copy of a draft, by Message-ID, in any account's Drafts folder:
    /// (message id, account id).
    pub fn find_draft(&self, message_id: &str) -> Result<Option<(i64, i64)>> {
        let c = self.r();
        Ok(c
            .query_row(
                "SELECT m.id, m.account_id FROM messages m JOIN folders f ON f.id = m.folder_id
                 WHERE f.role = 'drafts' AND m.message_id = ? ORDER BY m.id DESC LIMIT 1",
                [message_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }

    pub fn has_message_id(&self, folder_id: i64, message_id: &str) -> Result<bool> {
        let c = self.r();
        let n: i64 = c.query_row(
            "SELECT COUNT(*) FROM messages WHERE folder_id = ? AND message_id = ?",
            params![folder_id, message_id],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    pub fn unified_unread(&self) -> Result<i64> {
        let c = self.r();
        Ok(c.query_row(
            "SELECT COUNT(*) FROM messages WHERE seen = 0 AND folder_id IN (SELECT id FROM folders WHERE role = 'inbox')",
            [],
            |r| r.get(0),
        )?)
    }

    /// Highest message id stored. Ids only grow as mail arrives, so this marks "now".
    pub fn max_message_id(&self) -> Result<i64> {
        let c = self.r();
        Ok(c.query_row("SELECT COALESCE(MAX(id), 0) FROM messages", [], |r| r.get(0))?)
    }

    /// Unread inbox messages stored after the given id (excluding messages silenced by SkipNotification).
    pub fn unified_unread_after(&self, after_id: i64) -> Result<i64> {
        let c = self.r();
        Ok(c.query_row(
            "SELECT COUNT(*) FROM messages WHERE seen = 0 AND quiet = 0 AND id > ?1 AND folder_id IN (SELECT id FROM folders WHERE role = 'inbox')",
            [after_id],
            |r| r.get(0),
        )?)
    }

    /// Address suggestions, ranked by how often you write to / hear from them.
    pub fn contacts(&self, q: &str) -> Result<Vec<Addr>> {
        let c = self.r();
        let like = like_any(q);
        let mut st = c.prepare(
            "SELECT COALESCE(MAX(NULLIF(name, '')), ''), email, SUM(w) AS score FROM (
               SELECT from_name AS name, from_addr AS email, 1 AS w FROM messages
               UNION ALL
               SELECT json_extract(j.value, '$.name'), json_extract(j.value, '$.email'), 4
               FROM messages m, json_each(m.to_json) j
               WHERE m.folder_id IN (SELECT id FROM folders WHERE role = 'sent')
             )
             WHERE email != '' AND (email LIKE ?1 ESCAPE '\\' OR name LIKE ?1 ESCAPE '\\')
             GROUP BY lower(email) ORDER BY score DESC LIMIT 8",
        )?;
        let rows = st
            .query_map([like], |r| {
                let name: String = r.get(0)?;
                Ok(Addr { name: if name.is_empty() { None } else { Some(name) }, email: r.get(1)? })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Distinct lowercased email addresses you've sent mail to (To or Cc in Sent folders).
    pub fn sent_recipients(&self) -> Result<Vec<String>> {
        let c = self.r();
        let mut st = c.prepare(
            "SELECT DISTINCT lower(json_extract(j.value, '$.email')) AS email
             FROM messages m, json_each(m.to_json) j
             WHERE m.folder_id IN (SELECT id FROM folders WHERE role = 'sent')
               AND email IS NOT NULL AND email != ''
             UNION
             SELECT DISTINCT lower(json_extract(j.value, '$.email')) AS email
             FROM messages m, json_each(m.cc_json) j
             WHERE m.folder_id IN (SELECT id FROM folders WHERE role = 'sent')
               AND email IS NOT NULL AND email != ''",
        )?;
        let rows = st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    // ---------- outbox ----------

    pub fn add_outbox(&self, account_id: i64, message_id: &str, payload: &str, raw: &[u8], envelope: &str, send_at: i64) -> Result<i64> {
        let c = self.c();
        c.execute(
            "INSERT INTO outbox (account_id, message_id, payload, raw, envelope, send_at) VALUES (?, ?, ?, ?, ?, ?)",
            params![account_id, message_id, payload, raw, envelope, send_at],
        )?;
        Ok(c.last_insert_rowid())
    }

    /// (id, account_id, payload, send_at, error) for everything in the outbox, soonest first.
    pub fn outbox(&self) -> Result<Vec<OutboxSummaryRow>> {
        let c = self.r();
        let rows = c
            .prepare("SELECT id, account_id, payload, send_at, error FROM outbox ORDER BY send_at, id")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    fn outbox_row(r: &rusqlite::Row) -> rusqlite::Result<OutboxRow> {
        Ok(OutboxRow {
            id: r.get(0)?,
            account_id: r.get(1)?,
            message_id: r.get(2)?,
            payload: r.get(3)?,
            raw: r.get(4)?,
            envelope: r.get(5)?,
            attempts: r.get(6)?,
        })
    }

    /// Claims every message due by `now` for sending. Claimed ones can't be cancelled.
    pub fn claim_due_outbox(&self, now: i64) -> Result<Vec<OutboxRow>> {
        let mut c = self.c();
        let tx = c.transaction()?;
        let rows: Vec<OutboxRow> = tx
            .prepare("SELECT id, account_id, message_id, payload, raw, envelope, attempts FROM outbox WHERE sending = 0 AND send_at <= ?")?
            .query_map([now], Self::outbox_row)?
            .collect::<rusqlite::Result<_>>()?;
        for r in &rows {
            tx.execute("UPDATE outbox SET sending = 1 WHERE id = ?", [r.id])?;
        }
        tx.commit()?;
        Ok(rows)
    }

    /// Takes a message out of the outbox, unless it is being sent right now.
    pub fn take_outbox(&self, id: i64) -> Result<Option<OutboxRow>> {
        let c = self.c();
        let row = c
            .query_row(
                "SELECT id, account_id, message_id, payload, raw, envelope, attempts FROM outbox WHERE id = ? AND sending = 0",
                [id],
                Self::outbox_row,
            )
            .optional()?;
        if row.is_some() {
            c.execute("DELETE FROM outbox WHERE id = ?", [id])?;
        }
        Ok(row)
    }

    pub fn delete_outbox(&self, id: i64) -> Result<()> {
        self.c().execute("DELETE FROM outbox WHERE id = ?", [id])?;
        Ok(())
    }

    pub fn outbox_failed(&self, id: i64, error: &str, retry_at: i64) -> Result<()> {
        self.c().execute(
            "UPDATE outbox SET sending = 0, attempts = attempts + 1, error = ?, send_at = ? WHERE id = ?",
            params![error, retry_at, id],
        )?;
        Ok(())
    }

    pub fn reschedule_outbox(&self, id: i64, send_at: i64) -> Result<bool> {
        Ok(self.c().execute("UPDATE outbox SET send_at = ?, error = NULL WHERE id = ? AND sending = 0", params![send_at, id])? > 0)
    }

    /// After a crash nothing is mid-send any more.
    pub fn reset_outbox_claims(&self) -> Result<()> {
        self.c().execute("UPDATE outbox SET sending = 0 WHERE sending = 1", [])?;
        Ok(())
    }

    // ---------- snoozes ----------

    pub fn add_snooze(&self, account_id: i64, message_id: &str, return_path: &str, wake_at: i64) -> Result<()> {
        self.c().execute(
            "INSERT INTO snoozes (account_id, message_id, return_path, wake_at) VALUES (?, ?, ?, ?)
             ON CONFLICT(account_id, message_id) DO UPDATE SET wake_at = excluded.wake_at, woke_at = NULL,
               return_path = CASE WHEN snoozes.woke_at IS NULL THEN snoozes.return_path ELSE excluded.return_path END",
            params![account_id, message_id, return_path, wake_at],
        )?;
        Ok(())
    }

    /// Moves a pending snooze's wake time (e.g. to now, to unsnooze).
    pub fn set_wake(&self, account_id: i64, message_id: &str, wake_at: i64) -> Result<bool> {
        Ok(self.c().execute(
            "UPDATE snoozes SET wake_at = ? WHERE account_id = ? AND message_id = ? AND woke_at IS NULL",
            params![wake_at, account_id, message_id],
        )? > 0)
    }

    pub fn due_snoozes(&self, now: i64) -> Result<Vec<DueSnooze>> {
        let c = self.r();
        let rows = c
            .prepare("SELECT id, account_id, message_id, return_path FROM snoozes WHERE woke_at IS NULL AND wake_at <= ?")?
            .query_map([now], |r| Ok(DueSnooze { id: r.get(0)?, account_id: r.get(1)?, message_id: r.get(2)?, return_path: r.get(3)? }))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn mark_woke(&self, id: i64, at: i64) -> Result<()> {
        self.c().execute("UPDATE snoozes SET woke_at = ? WHERE id = ?", params![at, id])?;
        Ok(())
    }

    /// The soonest moment the scheduler has work: an outbox message or a snooze coming due.
    pub fn next_due(&self) -> Result<Option<i64>> {
        let c = self.r();
        Ok(c.query_row(
            "SELECT MIN(t) FROM (SELECT MIN(send_at) AS t FROM outbox WHERE sending = 0
                                 UNION ALL SELECT MIN(wake_at) FROM snoozes WHERE woke_at IS NULL)",
            [],
            |r| r.get(0),
        )?)
    }

    // ---------- rules ----------

    fn rule_row(r: &rusqlite::Row) -> rusqlite::Result<Rule> {
        let def: String = r.get(1)?;
        let mut rule: Rule = serde_json::from_str(&def)
            .map_err(|e| rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e)))?;
        rule.id = r.get(0)?;
        rule.enabled = r.get(2)?;
        rule.origin = r.get(3)?;
        rule.needs_approval = r.get(4)?;
        rule.paused_reason = r.get(5)?;
        Ok(rule)
    }

    const RULE_COLS: &'static str = "id, def, enabled, origin, needs_approval, paused_reason";

    /// Every rule, in the order they run.
    pub fn rules(&self) -> Result<Vec<Rule>> {
        let c = self.r();
        let sql = format!("SELECT {} FROM rules ORDER BY position, id", Self::RULE_COLS);
        let rows = c.prepare(&sql)?.query_map([], Self::rule_row)?.collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn rule(&self, id: i64) -> Result<Option<Rule>> {
        let c = self.r();
        let sql = format!("SELECT {} FROM rules WHERE id = ?", Self::RULE_COLS);
        Ok(c.query_row(&sql, [id], Self::rule_row).optional()?)
    }

    /// Adds a rule at the end. Returns its id.
    pub fn insert_rule(&self, rule: &Rule, now: i64) -> Result<i64> {
        let c = self.c();
        c.execute(
            "INSERT INTO rules (position, enabled, def, origin, needs_approval, paused_reason, created_at, updated_at)
             VALUES ((SELECT COALESCE(MAX(position), 0) + 1 FROM rules), ?, ?, ?, ?, ?, ?, ?)",
            params![rule.enabled, serde_json::to_string(rule)?, rule.origin, rule.needs_approval, rule.paused_reason, now, now],
        )?;
        Ok(c.last_insert_rowid())
    }

    pub fn update_rule(&self, rule: &Rule, now: i64) -> Result<()> {
        let c = self.c();
        let n = c.execute(
            "UPDATE rules SET enabled = ?, def = ?, origin = ?, needs_approval = ?, paused_reason = ?, updated_at = ? WHERE id = ?",
            params![rule.enabled, serde_json::to_string(rule)?, rule.origin, rule.needs_approval, rule.paused_reason, now, rule.id],
        )?;
        if n == 0 {
            return Err(anyhow!("There is no rule {}", rule.id));
        }
        Ok(())
    }

    pub fn delete_rule(&self, id: i64) -> Result<bool> {
        let mut c = self.c();
        let tx = c.transaction()?;
        tx.execute("DELETE FROM rule_replies WHERE rule_id = ?", [id])?;
        let deleted = tx.execute("DELETE FROM rules WHERE id = ?", [id])? > 0;
        tx.commit()?;
        Ok(deleted)
    }

    pub fn reorder_rules(&self, ids: &[i64]) -> Result<()> {
        let mut c = self.c();
        let tx = c.transaction()?;
        for (i, id) in ids.iter().enumerate() {
            tx.execute("UPDATE rules SET position = ? WHERE id = ?", params![i as i64 + 1, id])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn set_rule_state(&self, id: i64, enabled: bool, needs_approval: bool, paused_reason: Option<&str>) -> Result<()> {
        let c = self.c();
        let n = c.execute(
            "UPDATE rules SET enabled = ?, needs_approval = ?, paused_reason = ? WHERE id = ?",
            params![enabled, needs_approval, paused_reason, id],
        )?;
        if n == 0 {
            return Err(anyhow!("There is no rule {id}"));
        }
        Ok(())
    }

    pub fn rules_waiting_for_approval(&self) -> Result<usize> {
        let c = self.r();
        Ok(c.query_row("SELECT COUNT(*) FROM rules WHERE needs_approval = 1", [], |r| r.get::<_, i64>(0))? as usize)
    }

    /// Hands new Inbox mail to the rule runner.
    pub fn queue_for_rules(&self, account_id: i64, ids: &[i64], now: i64) -> Result<()> {
        let mut c = self.c();
        let tx = c.transaction()?;
        for id in ids {
            tx.execute("INSERT OR IGNORE INTO rule_queue (message_id, account_id, queued_at) VALUES (?, ?, ?)", params![id, account_id, now])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The oldest queued messages: (message id, account id, queued at).
    pub fn rule_queue(&self, limit: usize) -> Result<Vec<(i64, i64, i64)>> {
        let c = self.r();
        let rows = c
            .prepare("SELECT message_id, account_id, queued_at FROM rule_queue ORDER BY queued_at, message_id LIMIT ?")?
            .query_map([limit as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn unqueue_for_rules(&self, ids: &[i64]) -> Result<()> {
        let c = self.c();
        for id in ids {
            c.execute("DELETE FROM rule_queue WHERE message_id = ?", [id])?;
        }
        Ok(())
    }

    /// Records that rules looked at this message. False if they already had.
    pub fn mark_rule_seen(&self, account_id: i64, msgid: &str, now: i64) -> Result<bool> {
        let c = self.c();
        Ok(c.execute("INSERT OR IGNORE INTO rule_seen (account_id, msgid, at) VALUES (?, ?, ?)", params![account_id, msgid, now])? > 0)
    }

    pub fn prune_rule_seen(&self, before: i64) -> Result<()> {
        let c = self.c();
        c.execute("DELETE FROM rule_seen WHERE at < ?", [before])?;
        c.execute("DELETE FROM rule_replies WHERE at < ?", [before])?;
        Ok(())
    }

    const FACT_COLS: &'static str = "m.id, m.account_id, m.folder_id, m.from_name, m.from_addr, m.to_json, m.cc_json, m.reply_to_json,
        m.subject, m.list_id, m.auto, m.has_attachments, m.has_body, m.seen, m.date, m.message_id, m.refs, f.role";

    fn facts_row(r: &rusqlite::Row) -> rusqlite::Result<crate::rules::Facts> {
        let json = |i: usize| -> rusqlite::Result<Vec<Addr>> { Ok(serde_json::from_str(&r.get::<_, String>(i)?).unwrap_or_default()) };
        Ok(crate::rules::Facts {
            id: r.get(0)?,
            account_id: r.get(1)?,
            folder_id: r.get(2)?,
            from_name: r.get(3)?,
            from_addr: r.get(4)?,
            to: json(5)?,
            cc: json(6)?,
            reply_to: json(7)?,
            subject: r.get(8)?,
            list_id: r.get(9)?,
            auto: r.get(10)?,
            has_attachments: r.get(11)?,
            has_body: r.get(12)?,
            seen: r.get(13)?,
            date: r.get(14)?,
            message_id: r.get(15)?,
            refs: r.get(16)?,
            role: r.get(17)?,
        })
    }

    /// What rules look at in a stored message.
    pub fn rule_facts(&self, id: i64) -> Result<Option<crate::rules::Facts>> {
        let c = self.r();
        let sql = format!("SELECT {} FROM messages m JOIN folders f ON f.id = m.folder_id WHERE m.id = ?", Self::FACT_COLS);
        Ok(c.query_row(&sql, [id], Self::facts_row).optional()?)
    }

    /// The newest stored messages in a folder, or in the Inbox of one or every account.
    pub fn rule_candidates(&self, account_id: Option<i64>, folder_id: Option<i64>, limit: usize) -> Result<Vec<crate::rules::Facts>> {
        let c = self.r();
        let (scope, arg): (&str, Option<i64>) = match (folder_id, account_id) {
            (Some(f), _) => ("m.folder_id = ?1", Some(f)),
            (None, Some(a)) => ("f.role = 'inbox' AND m.account_id = ?1", Some(a)),
            (None, None) => ("f.role = 'inbox' AND ?1 IS NULL", None),
        };
        let sql = format!(
            "SELECT {} FROM messages m JOIN folders f ON f.id = m.folder_id WHERE {scope} ORDER BY m.date DESC LIMIT ?2",
            Self::FACT_COLS
        );
        let rows = c.prepare(&sql)?.query_map(params![arg, limit as i64], Self::facts_row)?.collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// The body as plain text (quoted replies left out), if it has been downloaded.
    pub fn body_text(&self, id: i64) -> Result<Option<String>> {
        let c = self.r();
        let has: Option<bool> = c.query_row("SELECT has_body FROM messages WHERE id = ?", [id], |r| r.get(0)).optional()?;
        if has != Some(true) {
            return Ok(None);
        }
        Ok(Some(c.query_row("SELECT body FROM messages_fts WHERE rowid = ?", [id], |r| r.get(0)).optional()?.unwrap_or_default()))
    }

    pub fn list_id(&self, id: i64) -> Result<Option<String>> {
        let c = self.r();
        Ok(c.query_row("SELECT list_id FROM messages WHERE id = ?", [id], |r| r.get(0)).optional()?.flatten())
    }

    /// Stored copies of a message in a folder, by Message-ID.
    pub fn ids_by_message_id(&self, folder_id: i64, msgid: &str) -> Result<Vec<i64>> {
        let c = self.r();
        let rows = c
            .prepare("SELECT id FROM messages WHERE folder_id = ? AND message_id = ?")?
            .query_map(params![folder_id, msgid], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// Stored copies of a message anywhere in an account, by Message-ID.
    pub fn ids_in_account(&self, account_id: i64, msgid: &str) -> Result<Vec<i64>> {
        let c = self.r();
        let rows = c
            .prepare("SELECT id FROM messages WHERE account_id = ? AND message_id = ?")?
            .query_map(params![account_id, msgid], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn log_rule(&self, rule: &Rule, account_id: i64, msgid: Option<&str>, subject: &str, from_addr: &str, action: &RuleAction, now: i64) -> Result<i64> {
        let c = self.c();
        c.execute(
            "INSERT INTO rule_log (rule_id, rule_name, account_id, msgid, subject, from_addr, action, at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            params![rule.id, rule.name, account_id, msgid, subject, from_addr, serde_json::to_string(action)?, now],
        )?;
        // Keep the log from growing forever.
        c.execute("DELETE FROM rule_log WHERE id NOT IN (SELECT id FROM rule_log ORDER BY id DESC LIMIT 2000)", [])?;
        Ok(c.last_insert_rowid())
    }

    /// The newest log entries: (entry, its Message-ID).
    pub fn rule_activity(&self, limit: usize) -> Result<Vec<(RuleActivity, Option<String>)>> {
        self.rule_log_where("1 = 1 ORDER BY id DESC LIMIT ?", limit as i64)
    }

    pub fn rule_log_entry(&self, id: i64) -> Result<Option<(RuleActivity, Option<String>)>> {
        Ok(self.rule_log_where("id = ?", id)?.pop())
    }

    fn rule_log_where(&self, filter: &str, arg: i64) -> Result<Vec<(RuleActivity, Option<String>)>> {
        let c = self.r();
        let sql = format!("SELECT id, rule_id, rule_name, account_id, subject, from_addr, action, at, undone, msgid FROM rule_log WHERE {filter}");
        let rows = c
            .prepare(&sql)?
            .query_map([arg], |r| {
                let action: String = r.get(6)?;
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, Option<i64>>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    action,
                    r.get::<_, i64>(7)?,
                    r.get::<_, bool>(8)?,
                    r.get::<_, Option<String>>(9)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows
            .into_iter()
            .filter_map(|(id, rule_id, rule_name, account_id, subject, from_addr, action, at, undone, msgid)| {
                let action: RuleAction = serde_json::from_str(&action).ok()?;
                let can_undo = !undone && msgid.is_some() && !action.sends() && action != RuleAction::SkipNotification;
                Some((RuleActivity { id, rule_id, rule_name, account_id, subject, from_addr, action, at, undone, can_undo }, msgid))
            })
            .collect())
    }

    pub fn mark_rule_undone(&self, id: i64) -> Result<()> {
        let c = self.c();
        c.execute("UPDATE rule_log SET undone = 1 WHERE id = ?", [id])?;
        Ok(())
    }

    /// Forwards and replies a rule has sent since `since`.
    pub fn rule_sends_since(&self, rule_id: i64, since: i64) -> Result<i64> {
        let c = self.r();
        Ok(c.query_row(
            "SELECT COUNT(*) FROM rule_log WHERE rule_id = ? AND at >= ?
               AND (action LIKE '{\"type\":\"forward\"%' OR action LIKE '{\"type\":\"reply\"%')",
            params![rule_id, since],
            |r| r.get(0),
        )?)
    }

    /// Whether a rule auto-replied to this address since `since`.
    pub fn replied_since(&self, rule_id: i64, addr: &str, since: i64) -> Result<bool> {
        let c = self.r();
        Ok(c.query_row(
            "SELECT COUNT(*) > 0 FROM rule_replies WHERE rule_id = ? AND addr = ? AND at >= ?",
            params![rule_id, addr.to_lowercase(), since],
            |r| r.get(0),
        )?)
    }

    pub fn record_reply(&self, rule_id: i64, addr: &str, now: i64) -> Result<()> {
        let c = self.c();
        c.execute(
            "INSERT INTO rule_replies (rule_id, addr, at) VALUES (?, ?, ?) ON CONFLICT(rule_id, addr) DO UPDATE SET at = excluded.at",
            params![rule_id, addr.to_lowercase(), now],
        )?;
        Ok(())
    }

    // ---------- settings ----------

    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        let c = self.r();
        Ok(c.query_row("SELECT value FROM settings WHERE key = ?", [key], |r| r.get(0)).optional()?)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        let c = self.c();
        c.execute(
            "INSERT INTO settings (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn delete_setting(&self, key: &str) -> Result<()> {
        let c = self.c();
        c.execute("DELETE FROM settings WHERE key = ?", [key])?;
        Ok(())
    }

    pub fn ics_rsvp(&self, account_id: i64, event_uid: &str) -> Result<Option<String>> {
        let c = self.r();
        Ok(c.query_row(
            "SELECT status FROM ics_rsvps WHERE account_id = ? AND event_uid = ?",
            params![account_id, event_uid],
            |r| r.get(0),
        )
        .optional()?)
    }

    pub fn set_ics_rsvp(&self, account_id: i64, event_uid: &str, status: &str, now: i64) -> Result<()> {
        let c = self.c();
        c.execute(
            "INSERT INTO ics_rsvps (account_id, event_uid, status, updated_at) VALUES (?, ?, ?, ?)
             ON CONFLICT(account_id, event_uid) DO UPDATE SET status = excluded.status, updated_at = excluded.updated_at",
            params![account_id, event_uid, status, now],
        )?;
        Ok(())
    }

    // ---------- agent permissions & actions ----------

    pub fn agent_account_modes(&self) -> Result<std::collections::HashMap<i64, AgentMode>> {
        let c = self.r();
        let mut s = c.prepare("SELECT account_id, mode FROM agent_account_modes")?;
        let mut out = std::collections::HashMap::new();
        let rows = s.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        for r in rows {
            let (id, mode) = r?;
            out.insert(id, AgentMode::parse(&mode));
        }
        Ok(out)
    }

    pub fn set_agent_account_mode(&self, account_id: i64, mode: Option<AgentMode>) -> Result<()> {
        let c = self.c();
        match mode {
            Some(m) => {
                c.execute(
                    "INSERT INTO agent_account_modes (account_id, mode) VALUES (?, ?)
                     ON CONFLICT(account_id) DO UPDATE SET mode = excluded.mode",
                    params![account_id, m.as_str()],
                )?;
            }
            None => {
                c.execute("DELETE FROM agent_account_modes WHERE account_id = ?", [account_id])?;
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn insert_agent_action(
        &self,
        account_id: Option<i64>,
        kind: &str,
        summary: &str,
        payload: &str,
        preview: &str,
        status: &str,
        created_at: i64,
        resolved_at: Option<i64>,
    ) -> Result<i64> {
        let c = self.c();
        c.execute(
            "INSERT INTO agent_actions (account_id, kind, summary, payload, preview, status, created_at, resolved_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            params![account_id, kind, summary, payload, preview, status, created_at, resolved_at],
        )?;
        Ok(c.last_insert_rowid())
    }

    fn row_to_agent_action(r: &rusqlite::Row) -> rusqlite::Result<AgentAction> {
        let payload: String = r.get(4)?;
        let preview: String = r.get(5)?;
        let spec: AgentActionSpec = serde_json::from_str(&payload)
            .map_err(|e| rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(e)))?;
        let preview: AgentActionPreview = serde_json::from_str(&preview).unwrap_or_default();
        Ok(AgentAction {
            id: r.get(0)?,
            account_id: r.get(1)?,
            kind: r.get(2)?,
            summary: r.get(3)?,
            spec,
            preview,
            status: r.get(6)?,
            error: r.get(7)?,
            created_at: r.get(8)?,
            resolved_at: r.get(9)?,
        })
    }

    pub fn agent_action(&self, id: i64) -> Result<Option<AgentAction>> {
        let c = self.r();
        Ok(c.query_row(
            "SELECT id, account_id, kind, summary, payload, preview, status, error, created_at, resolved_at
             FROM agent_actions WHERE id = ?",
            [id],
            Self::row_to_agent_action,
        )
        .optional()?)
    }

    /// Pending actions (oldest first for review/batching) followed by recent resolved actions (newest first).
    pub fn agent_actions(&self, recent_limit: usize) -> Result<Vec<AgentAction>> {
        let c = self.r();
        let mut out = Vec::new();
        {
            let mut s = c.prepare(
                "SELECT id, account_id, kind, summary, payload, preview, status, error, created_at, resolved_at
                 FROM agent_actions WHERE status = 'pending' ORDER BY created_at ASC, id ASC",
            )?;
            let rows = s.query_map([], Self::row_to_agent_action)?;
            for r in rows {
                out.push(r?);
            }
        }
        if recent_limit > 0 {
            let mut s = c.prepare(
                "SELECT id, account_id, kind, summary, payload, preview, status, error, created_at, resolved_at
                 FROM agent_actions WHERE status != 'pending' ORDER BY COALESCE(resolved_at, created_at) DESC, id DESC LIMIT ?",
            )?;
            let rows = s.query_map([recent_limit as i64], Self::row_to_agent_action)?;
            for r in rows {
                out.push(r?);
            }
        }
        Ok(out)
    }

    pub fn pending_agent_action_ids(&self) -> Result<Vec<i64>> {
        let c = self.r();
        let mut s = c.prepare("SELECT id FROM agent_actions WHERE status = 'pending' ORDER BY created_at ASC, id ASC")?;
        let rows = s.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn pending_agent_actions_count(&self) -> Result<usize> {
        let c = self.r();
        let n: i64 = c.query_row("SELECT COUNT(*) FROM agent_actions WHERE status = 'pending'", [], |r| r.get(0))?;
        Ok(n.max(0) as usize)
    }

    /// Transitions a pending action to a final status. Returns false if it was not pending.
    pub fn resolve_agent_action(&self, id: i64, status: &str, error: Option<&str>, resolved_at: i64) -> Result<bool> {
        let c = self.c();
        let n = c.execute(
            "UPDATE agent_actions SET status = ?, error = ?, resolved_at = ? WHERE id = ? AND status = 'pending'",
            params![status, error, resolved_at, id],
        )?;
        Ok(n > 0)
    }

    pub fn reject_all_pending_agent_actions(&self, resolved_at: i64) -> Result<usize> {
        let c = self.c();
        let n = c.execute(
            "UPDATE agent_actions SET status = 'rejected', resolved_at = ? WHERE status = 'pending'",
            params![resolved_at],
        )?;
        Ok(n)
    }

    /// Keeps at most `keep` resolved rows so the activity table stays bounded.
    pub fn prune_resolved_agent_actions(&self, keep: usize) -> Result<()> {
        let c = self.c();
        c.execute(
            "DELETE FROM agent_actions WHERE status != 'pending' AND id NOT IN (
                SELECT id FROM agent_actions WHERE status != 'pending'
                ORDER BY COALESCE(resolved_at, created_at) DESC, id DESC LIMIT ?
            )",
            [keep as i64],
        )?;
        Ok(())
    }

    // ---------- sender pictures ----------

    /// The cached picture for a domain (None inside when it has none) and when to look again.
    pub fn sender_icon(&self, domain: &str) -> Result<Option<(Option<Icon>, i64)>> {
        let c = self.r();
        Ok(c.query_row("SELECT mime, data, logo, retry_at FROM sender_icons WHERE domain = ?", [domain], |r| {
            let mime: Option<String> = r.get(0)?;
            let icon = match mime {
                Some(mime) => Some(Icon { mime, data: r.get(1)?, logo: r.get(2)? }),
                None => None,
            };
            Ok((icon, r.get(3)?))
        })
        .optional()?)
    }

    pub fn set_sender_icon(&self, domain: &str, icon: Option<&Icon>, retry_at: i64) -> Result<()> {
        let c = self.c();
        c.execute(
            "INSERT INTO sender_icons (domain, mime, data, logo, retry_at) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(domain) DO UPDATE SET mime = excluded.mime, data = excluded.data, logo = excluded.logo, retry_at = excluded.retry_at",
            params![domain, icon.map(|i| &i.mime), icon.map(|i| &i.data), icon.is_some_and(|i| i.logo), retry_at],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("mailvern-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("mailvern.db")
    }

    fn version(path: &Path) -> i64 {
        Connection::open(path).unwrap().query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap()
    }

    fn account() -> Account {
        Account {
            id: 0,
            name: "Me".into(),
            email: "me@example.com".into(),
            imap_host: "imap.example.com".into(),
            imap_port: 993,
            smtp_host: "smtp.example.com".into(),
            smtp_port: 465,
            color: String::new(),
            signature: "<b>Me</b>".into(),
            position: 0,
            label: "Work".into(),
            ..Default::default()
        }
    }

    fn msg(uid: u32, seen: bool) -> NewMessage {
        NewMessage {
            uid,
            message_id: None,
            in_reply_to: None,
            refs: None,
            subject: format!("m{uid}"),
            from_name: String::new(),
            from_addr: "a@example.com".into(),
            to: vec![],
            cc: vec![],
            reply_to: vec![],
            date: uid as i64,
            seen,
            flagged: false,
            has_attachments: false,
            size: 0,
            list_id: None,
            auto: None,
        }
    }

    fn mail(uid: u32, mid: &str, irt: Option<&str>, refs: Option<&str>, date: i64) -> NewMessage {
        NewMessage {
            message_id: Some(mid.into()),
            in_reply_to: irt.map(String::from),
            refs: refs.map(String::from),
            date,
            ..msg(uid, false)
        }
    }

    fn thread_of(db: &Db, folder: i64, uid: u32) -> String {
        db.c().query_row("SELECT thread_id FROM messages WHERE folder_id = ? AND uid = ?", params![folder, uid], |r| r.get(0)).unwrap()
    }

    #[test]
    fn replies_join_their_conversation_in_any_arrival_order() {
        let db = Db::open(&temp_db()).unwrap();
        let acc = db.save_account(&account()).unwrap();
        let inbox = db.upsert_folder(acc, "INBOX", "Inbox", Some("inbox")).unwrap();
        let sent = db.upsert_folder(acc, "Sent", "Sent", Some("sent")).unwrap();

        // A reply that only has In-Reply-To arrives before its parent.
        db.insert_messages(acc, inbox, &[mail(3, "<c>", Some("<b>"), None, 30)]).unwrap();
        // The parent refers to the root, which we never see.
        db.insert_messages(acc, sent, &[mail(1, "<b>", Some("<a>"), Some("<a>"), 20)]).unwrap();
        // An unrelated message.
        db.insert_messages(acc, inbox, &[mail(4, "<x>", None, None, 40)]).unwrap();
        // A later reply quoting the full chain.
        db.insert_messages(acc, inbox, &[mail(5, "<d>", Some("<c>"), Some("<a> <b> <c>"), 50)]).unwrap();

        let t = thread_of(&db, inbox, 3);
        assert_eq!(t, "<a>");
        assert_eq!(thread_of(&db, sent, 1), t);
        assert_eq!(thread_of(&db, inbox, 5), t);
        assert_eq!(thread_of(&db, inbox, 4), "<x>");

        // The inbox lists one row per conversation, standing for its inbox messages.
        let rows = db.list_threads(&View::Folder { folder_id: inbox }, 0, 50).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].subject, "m5");
        assert_eq!(rows[0].ids.len(), 2);
        assert_eq!(rows[0].count, 3, "count includes the Sent reply");
        assert!(!rows[0].seen);
        assert_eq!(rows[1].count, 1);

        // The conversation spans folders, oldest first.
        let thread = db.thread(rows[0].id).unwrap();
        assert_eq!(thread.iter().map(|m| m.subject.as_str()).collect::<Vec<_>>(), vec!["m1", "m3", "m5"]);
    }

    #[test]
    fn existing_mail_gets_conversations_on_upgrade() {
        let path = temp_db();
        {
            let db = Db::open(&path).unwrap();
            let acc = db.save_account(&account()).unwrap();
            let inbox = db.upsert_folder(acc, "INBOX", "Inbox", Some("inbox")).unwrap();
            db.insert_messages(acc, inbox, &[mail(1, "<a>", None, None, 1), mail(2, "<b>", Some("<a>"), None, 2)]).unwrap();
            db.c().execute("UPDATE messages SET thread_id = NULL", []).unwrap();
        }
        let db = Db::open(&path).unwrap();
        let n: i64 = db.c().query_row("SELECT COUNT(DISTINCT thread_id) FROM messages WHERE thread_id IS NOT NULL", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn search_hits_stay_out_of_the_sync_window() {
        let db = Db::open(&temp_db()).unwrap();
        let acc = db.save_account(&account()).unwrap();
        let inbox = db.upsert_folder(acc, "INBOX", "Inbox", Some("inbox")).unwrap();
        db.insert_messages(acc, inbox, &[msg(500, true), msg(501, true)]).unwrap();
        db.insert_messages_as(acc, inbox, &[msg(7, true)], true).unwrap();

        assert_eq!(db.uid_range(inbox).unwrap(), (500, 501));
        assert_eq!(db.sparse_uids_below(inbox, 500).unwrap(), vec![7]);
        assert_eq!(db.missing_uids(inbox, &[7, 8, 500]).unwrap(), vec![8]);

        // The server no longer has it.
        assert!(db.reconcile_uids(inbox, &[7], &[], &Default::default()).unwrap());
        assert!(db.sparse_uids_below(inbox, 500).unwrap().is_empty());

        // Sync scrolling down to a hit takes it into the window.
        db.insert_messages_as(acc, inbox, &[msg(499, true)], true).unwrap();
        db.insert_messages(acc, inbox, &[msg(499, true)]).unwrap();
        assert_eq!(db.uid_range(inbox).unwrap(), (499, 501));
    }

    #[test]
    fn search_stays_in_its_scope() {
        let db = Db::open(&temp_db()).unwrap();
        let a = db.save_account(&account()).unwrap();
        let b = db.save_account(&Account { email: "other@example.com".into(), ..account() }).unwrap();
        let a_inbox = db.upsert_folder(a, "INBOX", "Inbox", Some("inbox")).unwrap();
        let a_work = db.upsert_folder(a, "Work", "Work", None).unwrap();
        let b_inbox = db.upsert_folder(b, "INBOX", "Inbox", Some("inbox")).unwrap();
        let invoice = |uid| NewMessage { subject: "Invoice due".into(), ..msg(uid, true) };
        db.insert_messages(a, a_inbox, &[invoice(1)]).unwrap();
        db.insert_messages(a, a_work, &[invoice(2)]).unwrap();
        db.insert_messages(b, b_inbox, &[invoice(3), msg(4, true)]).unwrap();

        let hits = |scope: SearchScope| {
            let mut f: Vec<i64> = db.search("invoice", &scope, 50).unwrap().into_iter().map(|m| m.folder_id).collect();
            f.sort();
            f
        };
        assert_eq!(hits(SearchScope::All), vec![a_inbox, a_work, b_inbox]);
        assert_eq!(hits(SearchScope::Account { account_id: a }), vec![a_inbox, a_work]);
        assert_eq!(hits(SearchScope::Unified { role: "inbox".into() }), vec![a_inbox, b_inbox]);
        assert_eq!(hits(SearchScope::Folder { folder_id: a_work }), vec![a_work]);
        assert!(db.search("invoice", &SearchScope::Unified { role: "bogus".into() }, 50).is_err());
    }

    #[test]
    fn structured_queries_filter_count_and_group() {
        let db = Db::open(&temp_db()).unwrap();
        let a = db.save_account(&account()).unwrap();
        let inbox = db.upsert_folder(a, "INBOX", "Inbox", Some("inbox")).unwrap();
        let all = db.upsert_folder(a, "[Gmail]/All Mail", "All Mail", Some("all")).unwrap();
        let sent = db.upsert_folder(a, "Sent", "Sent", Some("sent")).unwrap();
        let from = |uid, name: &str, addr: &str, subject: &str, date| NewMessage {
            message_id: Some(format!("<m{uid}>")),
            from_name: name.into(),
            from_addr: addr.into(),
            subject: subject.into(),
            date,
            ..msg(uid, uid % 2 == 0)
        };
        db.insert_messages(a, inbox, &[
            from(1, "Tokopedia", "noreply@tokopedia.com", "Invoice INV/2026/11", 100),
            from(2, "Tokopedia", "noreply@tokopedia.com", "Pesanan dikirim", 200),
            NewMessage { has_attachments: true, ..from(3, "Budi", "budi@contoh.id", "Notulen rapat", 300) },
        ]).unwrap();
        // The same message again in All Mail: counted once.
        db.insert_messages(a, all, &[from(1, "Tokopedia", "noreply@tokopedia.com", "Invoice INV/2026/11", 100)]).unwrap();
        db.insert_messages(a, sent, &[NewMessage {
            to: vec![Addr { name: Some("Budi".into()), email: "budi@contoh.id".into() }],
            ..from(9, "Me", "me@example.com", "Re: Notulen rapat", 400)
        }]).unwrap();
        db.store_body(inbox, 3, b"", &BodyInfo { snippet: "".into(), search_text: "agenda anggaran kuartal".into(), has_attachments: true }).unwrap();

        let q = |q: MailQuery| db.query_mail(&q).unwrap();
        let subjects = |r: MailQueryResult| r.rows.into_iter().map(|m| m.subject).collect::<Vec<_>>();

        let r = q(MailQuery { from: Some("tokopedia".into()), ..Default::default() });
        assert_eq!(r.total, 2);
        assert_eq!(subjects(r), vec!["Pesanan dikirim", "Invoice INV/2026/11"]);
        assert_eq!(q(MailQuery { text: Some("anggaran".into()), ..Default::default() }).total, 1, "body words");
        assert_eq!(q(MailQuery { subject: Some("rapat".into()), ..Default::default() }).total, 2);
        assert_eq!(q(MailQuery { subject: Some("anggaran".into()), ..Default::default() }).total, 0, "subject only");
        assert_eq!(q(MailQuery { to: Some("budi".into()), ..Default::default() }).total, 1);
        assert_eq!(q(MailQuery { to: Some("email".into()), ..Default::default() }).total, 0, "JSON keys don't match to:");
        assert_eq!(q(MailQuery { after: Some(150), before: Some(300), ..Default::default() }).total, 1);
        assert_eq!(q(MailQuery { has_attachments: Some(true), ..Default::default() }).total, 1);
        assert_eq!(q(MailQuery { unread: Some(true), role: Some("inbox".into()), ..Default::default() }).total, 2);
        assert_eq!(q(MailQuery { role: Some("all".into()), ..Default::default() }).total, 1);
        assert_eq!(q(MailQuery { from: Some("100%_".into()), ..Default::default() }).total, 0, "LIKE wildcards are literal");
        // A server hit counts as matching the words even when the local copy has no body.
        let id = q(MailQuery { subject: Some("pesanan".into()), ..Default::default() }).rows[0].id;
        let r = q(MailQuery { text: Some("resi".into()), server_hits: vec![id], ..Default::default() });
        assert_eq!(subjects(r), vec!["Pesanan dikirim"]);
        // Paging and order.
        let r = q(MailQuery { limit: Some(2), offset: Some(1), oldest_first: true, ..Default::default() });
        assert_eq!(r.total, 4);
        assert_eq!(subjects(r), vec!["Pesanan dikirim", "Notulen rapat"]);
        assert!(db.query_mail(&MailQuery { role: Some("bogus".into()), ..Default::default() }).is_err());

        let (total, groups) = db.group_mail(&MailQuery::default(), "sender").unwrap();
        assert_eq!(total, 4);
        assert_eq!((groups[0].key.as_str(), groups[0].count, groups[0].label.as_deref()), ("noreply@tokopedia.com", 2, Some("Tokopedia")));
        let (_, groups) = db.group_mail(&MailQuery { role: Some("sent".into()), ..Default::default() }, "recipient").unwrap();
        assert_eq!((groups[0].key.as_str(), groups[0].count), ("budi@contoh.id", 1));
        let (_, groups) = db.group_mail(&MailQuery::default(), "domain").unwrap();
        assert_eq!(groups[0].key, "tokopedia.com");
        assert!(db.group_mail(&MailQuery::default(), "bogus").is_err());
    }

    #[test]
    fn outbox_claims_and_cancels() {
        let db = Db::open(&temp_db()).unwrap();
        let acc = db.save_account(&account()).unwrap();
        let a = db.add_outbox(acc, "<1>", "{}", b"raw", "{}", 100).unwrap();
        let b = db.add_outbox(acc, "<2>", "{}", b"raw", "{}", 200).unwrap();
        assert_eq!(db.next_due().unwrap(), Some(100));

        let due = db.claim_due_outbox(150).unwrap();
        assert_eq!(due.iter().map(|r| r.id).collect::<Vec<_>>(), vec![a]);
        // Being sent: can't be taken back, and isn't claimed twice.
        assert!(db.take_outbox(a).unwrap().is_none());
        assert!(db.claim_due_outbox(150).unwrap().is_empty());
        assert_eq!(db.next_due().unwrap(), Some(200));

        db.outbox_failed(a, "nope", 300).unwrap();
        assert_eq!(db.outbox().unwrap()[1].4.as_deref(), Some("nope"));
        assert!(db.take_outbox(b).unwrap().is_some());
        assert_eq!(db.outbox().unwrap().len(), 1);
    }

    #[test]
    fn snoozes_come_due_and_bump_the_message() {
        let db = Db::open(&temp_db()).unwrap();
        let acc = db.save_account(&account()).unwrap();
        let inbox = db.upsert_folder(acc, "INBOX", "Inbox", Some("inbox")).unwrap();
        db.insert_messages(acc, inbox, &[mail(1, "<old>", None, None, 10), mail(2, "<new>", None, None, 20)]).unwrap();

        db.add_snooze(acc, "<old>", "INBOX", 500).unwrap();
        let rows = db.list_threads(&View::Folder { folder_id: inbox }, 0, 10).unwrap();
        assert_eq!(rows.iter().find(|r| r.subject == "m1").unwrap().snoozed_until, Some(500));
        assert!(db.due_snoozes(499).unwrap().is_empty());
        let due = db.due_snoozes(500).unwrap();
        assert_eq!(due.len(), 1);
        db.mark_woke(due[0].id, 600).unwrap();

        // Back from snooze, it sorts as fresh.
        let rows = db.list_threads(&View::Folder { folder_id: inbox }, 0, 10).unwrap();
        assert_eq!(rows[0].subject, "m1");
        assert_eq!(rows[0].snoozed_until, None);
        assert_eq!(db.next_due().unwrap(), None);
    }

    #[test]
    fn badge_counts_only_unread_inbox_mail_after_the_mark() {
        let db = Db::open(&temp_db()).unwrap();
        let acc = db.save_account(&account()).unwrap();
        let inbox = db.upsert_folder(acc, "INBOX", "Inbox", Some("inbox")).unwrap();
        let other = db.upsert_folder(acc, "Lists", "Lists", None).unwrap();

        db.insert_messages(acc, inbox, &[msg(1, false), msg(2, true)]).unwrap();
        let mark = db.max_message_id().unwrap();
        // Already-seen-in-app mail stays unread but isn't new.
        assert_eq!(db.unified_unread().unwrap(), 1);
        assert_eq!(db.unified_unread_after(mark).unwrap(), 0);

        db.insert_messages(acc, inbox, &[msg(3, false), msg(4, true)]).unwrap();
        db.insert_messages(acc, other, &[msg(5, false)]).unwrap();
        assert_eq!(db.unified_unread_after(mark).unwrap(), 1);
        assert_eq!(db.unified_unread_after(db.max_message_id().unwrap()).unwrap(), 0);
    }

    #[test]
    fn fresh_database_is_current_and_not_backed_up() {
        let path = temp_db();
        Db::open(&path).unwrap();
        assert_eq!(version(&path), SCHEMA_VERSION);
        assert!(!path.with_extension("v0.bak").exists());
    }

    #[test]
    fn reopening_keeps_data() {
        let path = temp_db();
        Db::open(&path).unwrap().save_account(&account()).unwrap();
        let db = Db::open(&path).unwrap();
        let accs = db.accounts().unwrap();
        assert_eq!(accs.len(), 1);
        assert_eq!(accs[0].signature, "<b>Me</b>");
        assert_eq!(accs[0].label, "Work");
    }

    #[test]
    fn unversioned_database_from_first_build_is_adopted_with_backup() {
        let path = temp_db();
        // A database written before versioning existed: v1 tables, user_version 0.
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(V1).unwrap();
            conn.execute(
                "INSERT INTO accounts (name, email, imap_host, imap_port, smtp_host, smtp_port) VALUES ('Me', 'me@example.com', 'i', 993, 's', 465)",
                [],
            )
            .unwrap();
        }

        let db = Db::open(&path).unwrap();
        assert_eq!(version(&path), SCHEMA_VERSION);
        assert_eq!(db.accounts().unwrap().len(), 1);
        assert!(path.with_extension("v0.bak").exists());
    }

    #[test]
    fn future_migration_upgrades_existing_data_after_backup() {
        let path = temp_db();
        Db::open(&path).unwrap().save_account(&account()).unwrap();

        let next = "ALTER TABLE accounts ADD COLUMN nickname TEXT NOT NULL DEFAULT 'none';";
        let mut conn = Connection::open(&path).unwrap();
        let mut all: Vec<&str> = MIGRATIONS.to_vec();
        all.push(next);
        migrate_with(&mut conn, &path, &all).unwrap();

        assert_eq!(version(&path), SCHEMA_VERSION + 1);
        let nick: String = conn.query_row("SELECT nickname FROM accounts", [], |r| r.get(0)).unwrap();
        assert_eq!(nick, "none");
        let email: String = conn.query_row("SELECT email FROM accounts", [], |r| r.get(0)).unwrap();
        assert_eq!(email, "me@example.com");
        // The pre-upgrade backup holds the old schema and the data.
        let bak = Connection::open(path.with_extension(format!("v{SCHEMA_VERSION}.bak"))).unwrap();
        let v: i64 = bak.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        let n: i64 = bak.query_row("SELECT COUNT(*) FROM accounts", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn v1_database_gains_label_and_keeps_accounts() {
        let path = temp_db();
        {
            let mut conn = Connection::open(&path).unwrap();
            migrate_with(&mut conn, &path, &[V1]).unwrap();
            conn.execute(
                "INSERT INTO accounts (name, email, imap_host, imap_port, smtp_host, smtp_port) VALUES ('Me', 'me@example.com', 'i', 993, 's', 465)",
                [],
            )
            .unwrap();
        }
        let db = Db::open(&path).unwrap();
        let accs = db.accounts().unwrap();
        assert_eq!(accs.len(), 1);
        assert_eq!(accs[0].label, "");
        assert_eq!(version(&path), SCHEMA_VERSION);
        assert!(path.with_extension("v1.bak").exists());
    }

    #[test]
    fn folder_and_account_order_persist() {
        let path = temp_db();
        let db = Db::open(&path).unwrap();
        let a1 = db.save_account(&account()).unwrap();
        let a2 = db.save_account(&Account { email: "two@example.com".into(), ..account() }).unwrap();
        let inbox = db.upsert_folder(a1, "INBOX", "Inbox", Some("inbox")).unwrap();
        let sent = db.upsert_folder(a1, "Sent", "Sent", Some("sent")).unwrap();
        let news = db.upsert_folder(a1, "News", "News", None).unwrap();
        let b_inbox = db.upsert_folder(a2, "INBOX", "Inbox", Some("inbox")).unwrap();

        // Default: role order, accounts in creation order.
        let ids: Vec<i64> = db.folders().unwrap().iter().map(|f| f.id).collect();
        assert_eq!(ids, vec![inbox, sent, news, b_inbox]);

        db.reorder_folders(a1, &[news, inbox, sent]).unwrap();
        db.reorder_accounts(&[a2, a1]).unwrap();
        // A re-sync upserts folders again; the order must survive it.
        db.upsert_folder(a1, "News", "News", None).unwrap();

        let ids: Vec<i64> = Db::open(&path).unwrap().folders().unwrap().iter().map(|f| f.id).collect();
        assert_eq!(ids, vec![b_inbox, news, inbox, sent]);
        // Another account's ids are ignored.
        db.reorder_folders(a2, &[b_inbox, news]).unwrap();
        assert_eq!(db.folders().unwrap().iter().find(|f| f.id == news).unwrap().position, Some(0));
    }

    #[test]
    fn v4_database_gains_rules_and_keeps_mail() {
        let path = temp_db();
        {
            let mut conn = Connection::open(&path).unwrap();
            migrate_with(&mut conn, &path, &[V1, V2, V3, V4]).unwrap();
            conn.execute("INSERT INTO accounts (name, email, imap_host, imap_port, smtp_host, smtp_port) VALUES ('Me', 'me@x.com', 'i', 993, 's', 465)", []).unwrap();
            conn.execute("INSERT INTO folders (account_id, path, name, role) VALUES (1, 'INBOX', 'INBOX', 'inbox')", []).unwrap();
            conn.execute("INSERT INTO messages (account_id, folder_id, uid, subject) VALUES (1, 1, 1, 'Old')", []).unwrap();
        }
        let db = Db::open(&path).unwrap();
        assert_eq!(version(&path), SCHEMA_VERSION);
        let f = db.rule_facts(1).unwrap().unwrap();
        assert_eq!((f.subject.as_str(), f.list_id, f.auto), ("Old", None, None));
        assert!(db.rules().unwrap().is_empty());
        // New mail stores the headers rules use.
        let m = NewMessage { list_id: Some("<l.x.com>".into()), auto: Some("auto-replied".into()), ..msg(2, false) };
        let ids = db.insert_messages(1, 1, &[m]).unwrap();
        let f = db.rule_facts(ids[0]).unwrap().unwrap();
        assert_eq!((f.list_id.as_deref(), f.auto.as_deref()), (Some("<l.x.com>"), Some("auto-replied")));
        assert!(db.insert_messages(1, 1, &[msg(2, false)]).unwrap().is_empty());
    }

    #[test]
    fn failed_migration_rolls_back() {
        let path = temp_db();
        Db::open(&path).unwrap().save_account(&account()).unwrap();
        let bad = "ALTER TABLE accounts ADD COLUMN x TEXT; THIS IS NOT SQL;";
        let mut conn = Connection::open(&path).unwrap();
        let mut all: Vec<&str> = MIGRATIONS.to_vec();
        all.push(bad);
        assert!(migrate_with(&mut conn, &path, &all).is_err());
        assert_eq!(version(&path), SCHEMA_VERSION);
        let cols: i64 = conn.query_row("SELECT COUNT(*) FROM pragma_table_info('accounts') WHERE name = 'x'", [], |r| r.get(0)).unwrap();
        assert_eq!(cols, 0);
    }

    #[test]
    fn newer_database_is_refused_untouched() {
        let path = temp_db();
        Db::open(&path).unwrap().save_account(&account()).unwrap();
        Connection::open(&path).unwrap().pragma_update(None, "user_version", SCHEMA_VERSION + 1).unwrap();

        let err = Db::open(&path).err().expect("should refuse").to_string();
        assert!(err.contains("newer version"));
        assert_eq!(version(&path), SCHEMA_VERSION + 1);
    }

    #[test]
    fn contacts_escapes_wildcards_and_folder_synced_tracks_empty_inbox() {
        let db = Db::open(&temp_db()).unwrap();
        let a = db.save_account(&account()).unwrap();
        let f = db.upsert_folder(a, "INBOX", "Inbox", Some("inbox")).unwrap();
        assert!(!db.folder_synced(f).unwrap());
        db.mark_folder_synced(f).unwrap();
        assert!(db.folder_synced(f).unwrap());
        db.reset_folder(f, 42).unwrap();
        assert!(!db.folder_synced(f).unwrap());

        let m1 = NewMessage { from_name: "A_B".into(), from_addr: "a_b@example.com".into(), ..msg(1, false) };
        let m2 = NewMessage { from_name: "AXB".into(), from_addr: "axb@example.com".into(), ..msg(2, false) };
        db.insert_messages(a, f, &[m1, m2]).unwrap();
        let hits = db.contacts("a_b").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].email, "a_b@example.com");
    }

    #[test]
    fn sent_recipients_collects_unique_lowercased_to_and_cc_from_sent_only() {
        let db = Db::open(&temp_db()).unwrap();
        let a = db.save_account(&account()).unwrap();
        let inbox = db.upsert_folder(a, "INBOX", "Inbox", Some("inbox")).unwrap();
        let sent = db.upsert_folder(a, "Sent", "Sent", Some("sent")).unwrap();

        let inbox_msg = NewMessage {
            to: vec![Addr { name: None, email: "ignored@example.com".into() }],
            cc: vec![Addr { name: None, email: "ignored-cc@example.com".into() }],
            ..msg(1, false)
        };
        let sent_msg1 = NewMessage {
            to: vec![
                Addr { name: Some("Alice".into()), email: "Alice@Example.com".into() },
                Addr { name: None, email: "".into() },
            ],
            cc: vec![Addr { name: None, email: "Bob@Work.org".into() }],
            ..msg(2, true)
        };
        let sent_msg2 = NewMessage {
            to: vec![Addr { name: None, email: "alice@example.com".into() }],
            cc: vec![],
            ..msg(3, true)
        };
        db.insert_messages(a, inbox, &[inbox_msg]).unwrap();
        db.insert_messages(a, sent, &[sent_msg1, sent_msg2]).unwrap();

        let mut got = db.sent_recipients().unwrap();
        got.sort();
        assert_eq!(got, vec!["alice@example.com".to_string(), "bob@work.org".to_string()]);
    }

    #[test]
    fn v6_database_gains_agent_tables_and_cascades_on_account_delete() {
        let path = temp_db();
        {
            let mut conn = Connection::open(&path).unwrap();
            migrate_with(&mut conn, &path, &[V1, V2, V3, V4, V5, V6]).unwrap();
            conn.execute(
                "INSERT INTO accounts (name, email, imap_host, imap_port, smtp_host, smtp_port) VALUES ('Me', 'me@x.com', 'i', 993, 's', 465)",
                [],
            )
            .unwrap();
        }
        let db = Db::open(&path).unwrap();
        assert_eq!(version(&path), SCHEMA_VERSION);
        db.set_agent_account_mode(1, Some(AgentMode::Auto)).unwrap();
        assert_eq!(db.agent_account_modes().unwrap().get(&1), Some(&AgentMode::Auto));
        let spec = serde_json::to_string(&AgentActionSpec::Archive { ids: vec![1] }).unwrap();
        let preview = serde_json::to_string(&AgentActionPreview::default()).unwrap();
        let id = db.insert_agent_action(Some(1), "archive", "Archive 1 message", &spec, &preview, "pending", 100, None).unwrap();
        assert_eq!(db.pending_agent_actions_count().unwrap(), 1);
        assert!(db.agent_action(id).unwrap().is_some());

        // Deleting the account cascades to both tables.
        db.delete_account(1).unwrap();
        assert!(db.agent_account_modes().unwrap().is_empty());
        assert_eq!(db.pending_agent_actions_count().unwrap(), 0);
    }

    #[test]
    fn v7_upgrades_to_v8_and_persists_sync_state_and_pending_ops() {
        let path = temp_db();
        {
            let mut conn = Connection::open(&path).unwrap();
            migrate_with(&mut conn, &path, &[V1, V2, V3, V4, V5, V6, V7]).unwrap();
            conn.execute(
                "INSERT INTO accounts (name, email, imap_host, imap_port, smtp_host, smtp_port) VALUES ('Me', 'me@x.com', 'i', 993, 's', 465)",
                [],
            )
            .unwrap();
            conn.execute("INSERT INTO folders (account_id, path, name, role, uidvalidity, synced) VALUES (1, 'INBOX', 'Inbox', 'inbox', 7, 1)", [])
                .unwrap();
        }
        let db = Db::open(&path).unwrap();
        assert_eq!(version(&path), SCHEMA_VERSION);
        let st = db.folder_sync_state(1).unwrap();
        assert_eq!((st.uidvalidity, st.synced, st.msg_count, st.highest_modseq), (7, true, -1, 0));

        let next_st = FolderSyncState {
            uidvalidity: 7,
            synced: true,
            uidnext: 50,
            msg_count: 12,
            unseen_count: 3,
            highest_modseq: 999,
            last_full_sync: 1000,
        };
        db.update_folder_sync_state(1, &next_st).unwrap();
        assert_eq!(db.folder_sync_state(1).unwrap(), next_st);

        let op_id = db
            .enqueue_pending_op(1, 1, &PendingOpPayload::Store { flag: "\\Seen".into(), add: true }, &[10, 11], 100)
            .unwrap();
        assert!(db.has_pending_ops_for_folder(1).unwrap());
        let (pending_uids, mark_all) = db.pending_uids_for_folder(1).unwrap();
        assert!(!mark_all && pending_uids.contains(&10) && pending_uids.contains(&11));
        assert_eq!(db.due_pending_ops(1, Some(1), 100).unwrap().len(), 1);
        db.fail_pending_op(op_id, 200).unwrap();
        assert!(db.due_pending_ops(1, Some(1), 150).unwrap().is_empty());
        assert_eq!(db.due_pending_ops(1, Some(1), 200).unwrap()[0].attempts, 1);
        db.delete_pending_op(op_id).unwrap();
        assert!(!db.has_pending_ops_for_folder(1).unwrap());
    }

    #[test]
    fn reconciliation_protects_pending_ops_and_computes_missing_uids() {
        let db = Db::open(&temp_db()).unwrap();
        let acc = db.save_account(&account()).unwrap();
        let inbox = db.upsert_folder(acc, "INBOX", "Inbox", Some("inbox")).unwrap();
        // Local has UID 1 (optimistically marked seen), UID 2 (unseen), UID 3 (removed on server).
        // UID 4 was optimistically archived locally (in pending_uids), UID 5 is genuinely missing locally.
        db.insert_messages(acc, inbox, &[msg(1, true), msg(2, false), msg(3, false)]).unwrap();
        let pending: std::collections::HashSet<u32> = [1, 4].into_iter().collect();
        let server_flags = vec![(1, false, false), (2, true, true), (4, false, false), (5, false, false)];

        let (changed, missing) = db.reconcile_flags_and_missing(inbox, 1, &server_flags, &pending, false).unwrap();
        assert!(changed);
        // UID 4 is pending move -> must NOT be resurrected; UID 5 is missing -> returned for healing.
        assert_eq!(missing, vec![5]);
        // UID 1 stays seen=true despite server saying false; UID 2 updated to seen=true, flagged=true; UID 3 deleted.
        let rows = db.list(&View::Folder { folder_id: inbox }, None, 0, 10).unwrap();
        assert_eq!(rows.len(), 2);
        let r1 = rows.iter().find(|r| r.subject == "m1").unwrap();
        let r2 = rows.iter().find(|r| r.subject == "m2").unwrap();
        assert!(r1.seen);
        assert!(r2.seen && r2.flagged);
    }

    #[test]
    fn condstore_apply_changed_and_existing_uids_work() {
        let db = Db::open(&temp_db()).unwrap();
        let acc = db.save_account(&account()).unwrap();
        let inbox = db.upsert_folder(acc, "INBOX", "Inbox", Some("inbox")).unwrap();
        db.insert_messages(acc, inbox, &[msg(10, false), msg(11, false), msg(12, false)]).unwrap();
        let empty = std::collections::HashSet::new();

        assert!(db.apply_changed_flags(inbox, &[(11, true, true)], &empty, false).unwrap());
        let present: std::collections::HashSet<u32> = [10, 11, 13].into_iter().collect();
        let (changed, missing) = db.reconcile_existing_uids(inbox, 10, &present, &empty).unwrap();
        assert!(changed);
        assert_eq!(missing, vec![13]);
        let rows = db.list(&View::Folder { folder_id: inbox }, None, 0, 10).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().find(|r| r.subject == "m11").unwrap().flagged);
    }

    #[test]
    fn bodies_dedup_across_folders_stash_on_move_and_prune_by_size() {
        let db = Db::open(&temp_db()).unwrap();
        let acc = db.save_account(&account()).unwrap();
        let inbox = db.upsert_folder(acc, "INBOX", "Inbox", Some("inbox")).unwrap();
        let archive = db.upsert_folder(acc, "Archive", "Archive", Some("archive")).unwrap();
        let label = db.upsert_folder(acc, "Work", "Work", None).unwrap();

        let ids = db.insert_messages(acc, inbox, &[mail(1, "<m1>", None, None, 100)]).unwrap();
        let raw = vec![b'x'; 1024];
        let info = BodyInfo { snippet: "hello".into(), search_text: "hello world".into(), has_attachments: false };
        db.store_bodies_batch(inbox, &[(1, raw.clone(), info)]).unwrap();

        // Cross-folder dedup (Gmail labels): Work has same Message-ID -> hydrates locally.
        db.insert_messages(acc, label, &[mail(7, "<m1>", None, None, 100)]).unwrap();
        assert_eq!(db.copy_existing_bodies_for_folder(acc, label).unwrap(), 1);
        let label_id = db.ids_for_uids(label, &[7]).unwrap()[0];
        assert_eq!(db.body(label_id).unwrap().unwrap(), raw);

        // Stash on move: deleting from Inbox preserves body for Archive copy.
        db.stash_moved_bodies(&ids, 500).unwrap();
        db.delete_messages(&ids).unwrap();
        db.insert_messages(acc, archive, &[mail(9, "<m1>", None, None, 100)]).unwrap();
        assert_eq!(db.copy_existing_bodies_for_folder(acc, archive).unwrap(), 1);
        let arch_id = db.ids_for_uids(archive, &[9]).unwrap()[0];
        assert_eq!(db.body(arch_id).unwrap().unwrap(), raw);

        // Prune evicts oldest bodies when account exceeds max_bytes (keeping snippet/FTS).
        assert!(db.prune_old_bodies(acc, 500).unwrap() >= 1);
    }

    #[test]
    fn prefetch_skips_attachments_and_oversized_messages() {
        let db = Db::open(&temp_db()).unwrap();
        let acc = db.save_account(&account()).unwrap();
        let inbox = db.upsert_folder(acc, "INBOX", "Inbox", Some("inbox")).unwrap();

        let mut plain = mail(1, "<p1>", None, None, 100);
        plain.size = 50_000;
        let mut with_att_hdr = mail(2, "<a2>", None, None, 200);
        with_att_hdr.has_attachments = true;
        with_att_hdr.size = 50_000;
        let mut oversized = mail(3, "<big3>", None, None, 300);
        oversized.size = 4 * 1024 * 1024 + 1;
        let mut hidden_att = mail(4, "<ha4>", None, None, 400);
        hidden_att.size = 100_000;

        let ids = db.insert_messages(acc, inbox, &[plain, with_att_hdr, oversized, hidden_att]).unwrap();
        let missing = db.missing_bodies_with_size(inbox, 100, 4 * 1024 * 1024).unwrap();
        // UID 2 (has_attachments=1) and UID 3 (>4MB) are excluded; UID 1 and UID 4 are eligible.
        assert_eq!(missing.iter().map(|(u, _)| *u).collect::<Vec<_>>(), vec![4, 1]);

        // If UID 4 turns out to have attachments on parse, store_bodies_batch does NOT store its body.
        let stored = db
            .store_bodies_batch(
                inbox,
                &[
                    (1, b"plain body".to_vec(), BodyInfo { snippet: "plain".into(), search_text: "plain".into(), has_attachments: false }),
                    (4, b"has pdf".to_vec(), BodyInfo { snippet: "pdf".into(), search_text: "pdf".into(), has_attachments: true }),
                ],
            )
            .unwrap();
        assert_eq!(stored, 1);
        assert!(db.body(ids[0]).unwrap().is_some());
        assert!(db.body(ids[3]).unwrap().is_none());
        // And future prefetch queries now skip UID 4 too.
        assert!(db.missing_bodies_with_size(inbox, 100, 4 * 1024 * 1024).unwrap().is_empty());
    }

    #[test]
    fn quiet_messages_are_excluded_from_new_badge_count() {
        let db = Db::open(&temp_db()).unwrap();
        let acc = db.save_account(&account()).unwrap();
        let inbox = db.upsert_folder(acc, "INBOX", "Inbox", Some("inbox")).unwrap();
        let mark = db.max_message_id().unwrap();
        let ids = db.insert_messages(acc, inbox, &[msg(1, false), msg(2, false)]).unwrap();
        assert_eq!(db.unified_unread_after(mark).unwrap(), 2);
        db.mark_quiet(ids[0]).unwrap();
        assert_eq!(db.unified_unread_after(mark).unwrap(), 1);
    }

    #[test]
    fn v9_aliases_reply_to_ics_rsvps_smart_views_and_folder_crud() {
        let db = Db::open(&temp_db()).unwrap();
        let mut a = account();
        a.aliases = vec![Alias { name: "Support".into(), email: "support@example.com".into() }];
        a.reply_to = "replies@example.com".into();
        let acc = db.save_account(&a).unwrap();
        let loaded = db.account(acc).unwrap();
        assert_eq!(loaded.aliases, a.aliases);
        assert_eq!(loaded.reply_to, "replies@example.com");

        // ICS RSVP persistence
        assert_eq!(db.ics_rsvp(acc, "evt-1").unwrap(), None);
        db.set_ics_rsvp(acc, "evt-1", "ACCEPTED", 1000).unwrap();
        assert_eq!(db.ics_rsvp(acc, "evt-1").unwrap().as_deref(), Some("ACCEPTED"));
        db.set_ics_rsvp(acc, "evt-1", "DECLINED", 2000).unwrap();
        assert_eq!(db.ics_rsvp(acc, "evt-1").unwrap().as_deref(), Some("DECLINED"));

        // Smart views: Unread & Flagged exclude trash/junk and deduplicate All Mail
        let inbox = db.upsert_folder(acc, "INBOX", "Inbox", Some("inbox")).unwrap();
        let trash = db.upsert_folder(acc, "Trash", "Trash", Some("trash")).unwrap();
        let custom = db.upsert_folder(acc, "Projects", "Projects", None).unwrap();
        let sub = db.upsert_folder(acc, "Projects/Alpha", "Alpha", None).unwrap();

        let mut m1 = msg(1, false);
        m1.flagged = true;
        let m2 = msg(2, true);
        let mut m3 = msg(3, false);
        m3.flagged = true;
        db.insert_messages(acc, inbox, &[m1, m2]).unwrap();
        db.insert_messages(acc, trash, &[m3]).unwrap();

        let counts = db.smart_counts().unwrap();
        assert_eq!(counts.flagged, 1);
        assert_eq!(counts.unread, 1);
        assert_eq!(db.list_threads(&View::Smart { name: "flagged".into(), account_id: None }, 0, 50).unwrap().len(), 1);
        assert_eq!(db.list_threads(&View::Smart { name: "unread".into(), account_id: None }, 0, 50).unwrap().len(), 1);

        // Folder rename cascades child paths, clear_folder_messages empties Trash, delete removes folder
        db.rename_folder_local(custom, "Clients", "Clients").unwrap();
        assert_eq!(db.folder(custom).unwrap().1, "Clients");
        assert_eq!(db.folder(sub).unwrap().1, "Clients/Alpha");

        db.clear_folder_messages(trash).unwrap();
        assert!(db.list(&View::Folder { folder_id: trash }, None, 0, 50).unwrap().is_empty());

        db.delete_folder_local(sub).unwrap();
        assert!(db.folder(sub).is_err());
    }
}
