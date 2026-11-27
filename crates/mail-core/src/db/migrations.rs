use anyhow::{anyhow, Context, Result};
use rusqlite::Connection;
use std::path::Path;


/// Caches that can be rebuilt from the network at any time, so they stay out of the
/// versioned schema: created on open, and dropping one only costs a refetch.
///
/// `sender_icons`: one row per sender domain. `mime` is NULL when the domain has no
/// picture. `retry_at` is when to look again.
pub(super) const CACHES: &str = r#"
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
pub(super) const MIGRATIONS: &[&str] = &[V1, V2, V3, V4, V5, V6, V7, V8, V9, V10, V11, V12];

/// v12: attachment filename search column, saved searches, and per-account/per-folder MCP read access.
pub(super) const V12: &str = r#"
ALTER TABLE messages ADD COLUMN attachment_names TEXT NOT NULL DEFAULT '';

CREATE TABLE IF NOT EXISTS saved_searches (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  query TEXT NOT NULL,
  scope_json TEXT NOT NULL DEFAULT '{"kind":"all"}',
  position INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS agent_account_access (
  account_id INTEGER PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
  allowed INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS agent_folder_access (
  folder_id INTEGER PRIMARY KEY REFERENCES folders(id) ON DELETE CASCADE,
  allowed INTEGER NOT NULL
);
"#;

/// v11: denormalized `sort_date` for indexed message/thread sorting, partial indexes for unread
/// and unassigned-thread lookups, deduplicated compressed `raw_blobs`, and incremental `contacts_cache`.
pub(super) const V11: &str = r#"
ALTER TABLE messages ADD COLUMN sort_date INTEGER NOT NULL DEFAULT 0;
UPDATE messages SET sort_date = MAX(
  date,
  COALESCE((SELECT s.woke_at FROM snoozes s WHERE s.account_id = messages.account_id AND s.message_id = messages.message_id), 0)
);
DROP INDEX IF EXISTS idx_messages_folder_date;
CREATE INDEX IF NOT EXISTS idx_messages_folder_sort ON messages(folder_id, sort_date DESC, id DESC);
CREATE INDEX IF NOT EXISTS idx_messages_unread ON messages(folder_id, account_id, message_id) WHERE seen = 0;
CREATE INDEX IF NOT EXISTS idx_messages_no_thread ON messages(id) WHERE thread_id IS NULL;

CREATE TABLE IF NOT EXISTS raw_blobs (
  id INTEGER PRIMARY KEY,
  account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  message_id TEXT,
  raw BLOB NOT NULL,
  UNIQUE(account_id, message_id)
);
ALTER TABLE bodies ADD COLUMN blob_id INTEGER REFERENCES raw_blobs(id) ON DELETE CASCADE;
CREATE INDEX IF NOT EXISTS idx_bodies_blob ON bodies(blob_id) WHERE blob_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS contacts_cache (
  email_lc TEXT PRIMARY KEY,
  email TEXT NOT NULL,
  name TEXT NOT NULL DEFAULT '',
  score INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX IF NOT EXISTS idx_contacts_score ON contacts_cache(score DESC);

INSERT OR REPLACE INTO contacts_cache (email_lc, email, name, score)
SELECT lower(email), MAX(email), COALESCE(MAX(NULLIF(name, '')), ''), SUM(w)
FROM (
  SELECT from_name AS name, from_addr AS email, 1 AS w FROM messages WHERE from_addr != ''
  UNION ALL
  SELECT COALESCE(json_extract(j.value, '$.name'), ''), json_extract(j.value, '$.email'), 4
  FROM messages m, json_each(m.to_json) j
  WHERE m.folder_id IN (SELECT id FROM folders WHERE role = 'sent')
    AND json_extract(j.value, '$.email') IS NOT NULL
    AND json_extract(j.value, '$.email') != ''
)
GROUP BY lower(email);
"#;

/// v10: per-account desktop notification toggle and Dock badge override.
pub(super) const V10: &str = r#"
ALTER TABLE accounts ADD COLUMN notify INTEGER NOT NULL DEFAULT 1;
ALTER TABLE accounts ADD COLUMN badge_mode TEXT NOT NULL DEFAULT '';
"#;

/// v9: send-as aliases and default Reply-To per account, flagged message index for smart views,
/// and persisted calendar invite (ICS) RSVP responses.
pub(super) const V9: &str = r#"
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
pub(super) const V8: &str = r#"
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
pub(super) const V7: &str = r#"
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
pub(super) const V6: &str = r#"
ALTER TABLE folders ADD COLUMN synced INTEGER NOT NULL DEFAULT 0;
UPDATE folders SET synced = 1 WHERE uidvalidity != 0 OR EXISTS (SELECT 1 FROM messages m WHERE m.folder_id = folders.id);
"#;

/// v5: rules.
///
/// `list_id` and `auto` come from headers, for rule conditions and loop protection.
/// `rule_queue` holds new Inbox mail waiting for the rule runner. `rule_seen` records the
/// Message-IDs rules have looked at, so a message that comes back to the Inbox (with a new
/// UID) isn't processed twice. `rule_replies` limits auto-replies to one per sender now and then.
pub(super) const V5: &str = r#"
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
pub(super) const V4: &str = r#"
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
pub(super) const V3: &str = "ALTER TABLE folders ADD COLUMN position INTEGER;";

/// v2: a sidebar label separate from the sender name.
pub(super) const V2: &str = "ALTER TABLE accounts ADD COLUMN label TEXT NOT NULL DEFAULT '';";

pub(super) const V1: &str = r#"
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

#[cfg(test)]
pub(super) const SCHEMA_VERSION: i64 = MIGRATIONS.len() as i64;

pub(super) fn migrate(conn: &mut Connection, path: &Path) -> Result<()> {
    migrate_with(conn, path, MIGRATIONS)
}

pub(super) fn migrate_with(conn: &mut Connection, path: &Path, migrations: &[&str]) -> Result<()> {
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
    let mut created_backup = None;
    if has_tables {
        let backup = path.with_extension(format!("v{current}.bak"));
        let _ = std::fs::remove_file(&backup);
        conn.execute("VACUUM INTO ?", [backup.to_string_lossy()])?;
        created_backup = Some(backup);
    } else {
        // Fresh database: enable incremental auto_vacuum before any tables are created.
        let _ = conn.execute_batch("PRAGMA auto_vacuum = INCREMENTAL;");
    }
    for (i, sql) in migrations.iter().enumerate().skip(current as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql).with_context(|| format!("Database upgrade to version {} failed", i + 1))?;
        tx.pragma_update(None, "user_version", (i + 1) as i64)?;
        tx.commit()?;
    }
    // Keep only the newest pre-upgrade backup so historical full DB copies don't pile up on disk.
    if let Some(keep) = created_backup {
        if let (Some(dir), Some(stem)) = (path.parent(), path.file_stem().and_then(|s| s.to_str())) {
            let prefix = format!("{stem}.v");
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p == keep {
                        continue;
                    }
                    if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                        if name.starts_with(&prefix) && name.ends_with(".bak") {
                            let _ = std::fs::remove_file(&p);
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
