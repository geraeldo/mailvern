use crate::model::*;
use anyhow::{anyhow, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::Mutex;

pub struct Db {
    conn: Mutex<Connection>,
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
}

/// Summary fields computed from a full message body.
pub struct BodyInfo {
    pub snippet: String,
    pub search_text: String,
    pub has_attachments: bool,
}

/// Per-connection settings. Not part of the schema; applied on every open.
const PRAGMAS: &str = "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;";

/// Schema history. `PRAGMA user_version` records how many of these have run.
///
/// Rules: never edit or reorder a shipped entry. To change the schema, append a new
/// entry (e.g. `ALTER TABLE messages ADD COLUMN ...`). Each one runs once, in a
/// transaction, and the database is backed up before any upgrade.
const MIGRATIONS: &[&str] = &[V1, V2, V3];

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
    "m.id, m.account_id, m.folder_id, m.from_name, m.from_addr, m.subject, m.snippet, m.date, m.seen, m.flagged, m.has_attachments";

fn row_to_summary(r: &rusqlite::Row) -> rusqlite::Result<MessageSummary> {
    Ok(MessageSummary {
        id: r.get(0)?,
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
    })
}

fn row_to_account(r: &rusqlite::Row) -> rusqlite::Result<Account> {
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
    })
}

/// Turns free text into a safe FTS5 prefix query.
fn fts_query(q: &str) -> Option<String> {
    let terms: Vec<String> = q
        .split_whitespace()
        .map(|t| t.replace('"', ""))
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{}\"*", t))
        .collect();
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" "))
    }
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
        Ok(Db { conn: Mutex::new(conn) })
    }

    fn c(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ---------- accounts ----------

    pub fn accounts(&self) -> Result<Vec<Account>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT id, name, email, imap_host, imap_port, smtp_host, smtp_port, color, signature, position, label
             FROM accounts ORDER BY position, id",
        )?;
        let rows = st.query_map([], row_to_account)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn account(&self, id: i64) -> Result<Account> {
        let c = self.c();
        Ok(c.query_row(
            "SELECT id, name, email, imap_host, imap_port, smtp_host, smtp_port, color, signature, position, label
             FROM accounts WHERE id = ?",
            [id],
            row_to_account,
        )?)
    }

    pub fn save_account(&self, a: &Account) -> Result<i64> {
        let c = self.c();
        if a.id > 0 {
            c.execute(
                "UPDATE accounts SET name=?, email=?, imap_host=?, imap_port=?, smtp_host=?, smtp_port=?, color=?, signature=?, label=? WHERE id=?",
                params![a.name, a.email, a.imap_host, a.imap_port, a.smtp_host, a.smtp_port, a.color, a.signature, a.label, a.id],
            )?;
            Ok(a.id)
        } else {
            let pos: i64 = c.query_row("SELECT COALESCE(MAX(position), 0) + 1 FROM accounts", [], |r| r.get(0))?;
            c.execute(
                "INSERT INTO accounts (name, email, imap_host, imap_port, smtp_host, smtp_port, color, signature, position, label)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![a.name, a.email, a.imap_host, a.imap_port, a.smtp_host, a.smtp_port, a.color, a.signature, pos, a.label],
            )?;
            Ok(c.last_insert_rowid())
        }
    }

    pub fn reorder_accounts(&self, ids: &[i64]) -> Result<()> {
        let c = self.c();
        for (i, id) in ids.iter().enumerate() {
            c.execute("UPDATE accounts SET position = ? WHERE id = ?", params![i as i64, id])?;
        }
        Ok(())
    }

    pub fn delete_account(&self, id: i64) -> Result<()> {
        let c = self.c();
        c.execute(
            "DELETE FROM messages_fts WHERE rowid IN (SELECT id FROM messages WHERE account_id = ?)",
            [id],
        )?;
        c.execute("DELETE FROM accounts WHERE id = ?", [id])?;
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

    pub fn remove_folders_except(&self, account_id: i64, keep: &[i64]) -> Result<()> {
        let c = self.c();
        let existing: Vec<i64> = c
            .prepare("SELECT id FROM folders WHERE account_id = ?")?
            .query_map([account_id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for id in existing.into_iter().filter(|id| !keep.contains(id)) {
            c.execute("DELETE FROM messages_fts WHERE rowid IN (SELECT id FROM messages WHERE folder_id = ?)", [id])?;
            c.execute("DELETE FROM folders WHERE id = ?", [id])?;
        }
        Ok(())
    }

    /// Folders in display order: accounts in their order, then each account's
    /// folders by user position, falling back to a sensible default.
    pub fn folders(&self) -> Result<Vec<Folder>> {
        let c = self.c();
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

    pub fn folder(&self, id: i64) -> Result<(i64, String, Option<String>)> {
        let c = self.c();
        Ok(c.query_row("SELECT account_id, path, role FROM folders WHERE id = ?", [id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?)
    }

    pub fn folder_by_role(&self, account_id: i64, role: &str) -> Result<Option<(i64, String)>> {
        let c = self.c();
        Ok(c.query_row(
            "SELECT id, path FROM folders WHERE account_id = ? AND role = ?",
            params![account_id, role],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
    }

    pub fn folder_uidvalidity(&self, folder_id: i64) -> Result<u32> {
        let c = self.c();
        let v: i64 = c.query_row("SELECT uidvalidity FROM folders WHERE id = ?", [folder_id], |r| r.get(0))?;
        Ok(v as u32)
    }

    pub fn reset_folder(&self, folder_id: i64, uidvalidity: u32) -> Result<()> {
        let c = self.c();
        c.execute("DELETE FROM messages_fts WHERE rowid IN (SELECT id FROM messages WHERE folder_id = ?)", [folder_id])?;
        c.execute("DELETE FROM messages WHERE folder_id = ?", [folder_id])?;
        c.execute("UPDATE folders SET uidvalidity = ? WHERE id = ?", params![uidvalidity as i64, folder_id])?;
        Ok(())
    }

    // ---------- messages ----------

    pub fn uid_range(&self, folder_id: i64) -> Result<(u32, u32)> {
        let c = self.c();
        let (min, max): (Option<i64>, Option<i64>) = c.query_row(
            "SELECT MIN(uid), MAX(uid) FROM messages WHERE folder_id = ?",
            [folder_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok((min.unwrap_or(0) as u32, max.unwrap_or(0) as u32))
    }

    pub fn insert_messages(&self, account_id: i64, folder_id: i64, msgs: &[NewMessage]) -> Result<()> {
        let mut c = self.c();
        let tx = c.transaction()?;
        {
            let mut ins = tx.prepare(
                "INSERT OR IGNORE INTO messages (account_id, folder_id, uid, message_id, in_reply_to, refs, subject,
                    from_name, from_addr, to_json, cc_json, reply_to_json, date, seen, flagged, has_attachments, size)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )?;
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
                ])?;
                if n > 0 {
                    let id = tx.last_insert_rowid();
                    fts.execute(params![id, m.subject, format!("{} {}", m.from_name, m.from_addr)])?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Applies server flags for every uid >= `from_uid` and deletes rows the server no longer has.
    pub fn reconcile_flags(&self, folder_id: i64, from_uid: u32, flags: &[(u32, bool, bool)]) -> Result<bool> {
        let mut c = self.c();
        let tx = c.transaction()?;
        let mut changed = false;
        {
            let local: Vec<(i64, u32, bool, bool)> = tx
                .prepare("SELECT id, uid, seen, flagged FROM messages WHERE folder_id = ? AND uid >= ?")?
                .query_map(params![folder_id, from_uid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                .collect::<rusqlite::Result<_>>()?;
            let server: std::collections::HashMap<u32, (bool, bool)> =
                flags.iter().map(|(u, s, f)| (*u, (*s, *f))).collect();
            for (id, uid, seen, flagged) in local {
                match server.get(&uid) {
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
        }
        tx.commit()?;
        Ok(changed)
    }

    pub fn missing_bodies(&self, folder_id: i64, limit: usize, max_size: u32) -> Result<Vec<u32>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT uid FROM messages WHERE folder_id = ? AND has_body = 0 AND size <= ? ORDER BY date DESC LIMIT ?",
        )?;
        let rows = st
            .query_map(params![folder_id, max_size, limit as i64], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<u32>>>()?;
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

    pub fn body(&self, id: i64) -> Result<Option<Vec<u8>>> {
        let c = self.c();
        Ok(c.query_row("SELECT raw FROM bodies WHERE message_id = ?", [id], |r| r.get(0)).optional()?)
    }

    /// (account_id, folder_id, folder_path, uid)
    pub fn locate(&self, id: i64) -> Result<(i64, i64, String, u32)> {
        let c = self.c();
        Ok(c.query_row(
            "SELECT m.account_id, m.folder_id, f.path, m.uid FROM messages m JOIN folders f ON f.id = m.folder_id WHERE m.id = ?",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?)
    }

    pub fn list(&self, view: &View, query: Option<&str>, offset: i64, limit: i64) -> Result<Vec<MessageSummary>> {
        let c = self.c();
        let scope = match view {
            View::Unified { role } => {
                let role = UNIFIED_ROLES.iter().find(|r| **r == role).ok_or_else(|| anyhow!("Unknown folder role"))?;
                format!("m.folder_id IN (SELECT id FROM folders WHERE role = '{role}')")
            }
            View::Folder { folder_id } => format!("m.folder_id = {}", folder_id),
        };
        let fts = query.and_then(fts_query);
        let sql = match fts {
            Some(_) => format!(
                "SELECT {SUMMARY_COLS} FROM messages m JOIN messages_fts ON messages_fts.rowid = m.id
                 WHERE {scope} AND messages_fts MATCH ?1 ORDER BY m.date DESC LIMIT ?2 OFFSET ?3"
            ),
            None => format!(
                "SELECT {SUMMARY_COLS} FROM messages m WHERE {scope} AND ?1 IS NULL ORDER BY m.date DESC LIMIT ?2 OFFSET ?3"
            ),
        };
        let mut st = c.prepare(&sql)?;
        let rows = st
            .query_map(params![fts, limit, offset], row_to_summary)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Searches every folder of every account.
    pub fn search_all(&self, query: &str, limit: i64) -> Result<Vec<MessageSummary>> {
        let Some(fts) = fts_query(query) else { return Ok(vec![]) };
        let c = self.c();
        let sql = format!(
            "SELECT {SUMMARY_COLS} FROM messages m JOIN messages_fts ON messages_fts.rowid = m.id
             WHERE messages_fts MATCH ?1 ORDER BY m.date DESC LIMIT ?2"
        );
        let mut st = c.prepare(&sql)?;
        let rows = st.query_map(params![fts, limit], row_to_summary)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn summary(&self, id: i64) -> Result<(MessageSummary, Vec<Addr>, Vec<Addr>, Vec<Addr>, Option<String>, Option<String>, Option<String>)> {
        let c = self.c();
        let sql = format!(
            "SELECT {SUMMARY_COLS}, m.to_json, m.cc_json, m.reply_to_json, m.message_id, m.refs, m.in_reply_to FROM messages m WHERE m.id = ?"
        );
        Ok(c.query_row(&sql, [id], |r| {
            let s = row_to_summary(r)?;
            let to: String = r.get(11)?;
            let cc: String = r.get(12)?;
            let rt: String = r.get(13)?;
            Ok((
                s,
                serde_json::from_str(&to).unwrap_or_default(),
                serde_json::from_str(&cc).unwrap_or_default(),
                serde_json::from_str(&rt).unwrap_or_default(),
                r.get(14)?,
                r.get(15)?,
                r.get(16)?,
            ))
        })?)
    }

    pub fn set_seen(&self, ids: &[i64], seen: bool) -> Result<()> {
        let c = self.c();
        for id in ids {
            c.execute("UPDATE messages SET seen = ? WHERE id = ?", params![seen, id])?;
        }
        Ok(())
    }

    /// Marks every cached message in a folder as read.
    pub fn mark_folder_read(&self, folder_id: i64) -> Result<usize> {
        let c = self.c();
        Ok(c.execute("UPDATE messages SET seen = 1 WHERE folder_id = ? AND seen = 0", [folder_id])?)
    }

    pub fn set_flagged(&self, ids: &[i64], flagged: bool) -> Result<()> {
        let c = self.c();
        for id in ids {
            c.execute("UPDATE messages SET flagged = ? WHERE id = ?", params![flagged, id])?;
        }
        Ok(())
    }

    pub fn delete_messages(&self, ids: &[i64]) -> Result<()> {
        let c = self.c();
        for id in ids {
            c.execute("DELETE FROM messages_fts WHERE rowid = ?", [id])?;
            c.execute("DELETE FROM messages WHERE id = ?", [id])?;
        }
        Ok(())
    }

    pub fn has_message_id(&self, folder_id: i64, message_id: &str) -> Result<bool> {
        let c = self.c();
        let n: i64 = c.query_row(
            "SELECT COUNT(*) FROM messages WHERE folder_id = ? AND message_id = ?",
            params![folder_id, message_id],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    pub fn unified_unread(&self) -> Result<i64> {
        let c = self.c();
        Ok(c.query_row(
            "SELECT COUNT(*) FROM messages WHERE seen = 0 AND folder_id IN (SELECT id FROM folders WHERE role = 'inbox')",
            [],
            |r| r.get(0),
        )?)
    }

    /// Highest message id stored. Ids only grow as mail arrives, so this marks "now".
    pub fn max_message_id(&self) -> Result<i64> {
        let c = self.c();
        Ok(c.query_row("SELECT COALESCE(MAX(id), 0) FROM messages", [], |r| r.get(0))?)
    }

    /// Unread inbox messages stored after the given id.
    pub fn unified_unread_after(&self, after_id: i64) -> Result<i64> {
        let c = self.c();
        Ok(c.query_row(
            "SELECT COUNT(*) FROM messages WHERE seen = 0 AND id > ?1 AND folder_id IN (SELECT id FROM folders WHERE role = 'inbox')",
            [after_id],
            |r| r.get(0),
        )?)
    }

    /// Address suggestions, ranked by how often you write to / hear from them.
    pub fn contacts(&self, q: &str) -> Result<Vec<Addr>> {
        let c = self.c();
        let like = format!("%{}%", q.trim().replace('%', "").replace('_', ""));
        let mut st = c.prepare(
            "SELECT COALESCE(MAX(NULLIF(name, '')), ''), email, SUM(w) AS score FROM (
               SELECT from_name AS name, from_addr AS email, 1 AS w FROM messages
               UNION ALL
               SELECT json_extract(j.value, '$.name'), json_extract(j.value, '$.email'), 4
               FROM messages m, json_each(m.to_json) j
               WHERE m.folder_id IN (SELECT id FROM folders WHERE role = 'sent')
             )
             WHERE email != '' AND (email LIKE ?1 OR name LIKE ?1)
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

    // ---------- settings ----------

    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        let c = self.c();
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
        }
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
        migrate_with(&mut conn, &path, &[V1, V2, V3, next]).unwrap();

        assert_eq!(version(&path), 4);
        let nick: String = conn.query_row("SELECT nickname FROM accounts", [], |r| r.get(0)).unwrap();
        assert_eq!(nick, "none");
        let email: String = conn.query_row("SELECT email FROM accounts", [], |r| r.get(0)).unwrap();
        assert_eq!(email, "me@example.com");
        // The pre-upgrade backup holds the old schema and the data.
        let bak = Connection::open(path.with_extension("v3.bak")).unwrap();
        let v: i64 = bak.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 3);
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
    fn failed_migration_rolls_back() {
        let path = temp_db();
        Db::open(&path).unwrap().save_account(&account()).unwrap();
        let bad = "ALTER TABLE accounts ADD COLUMN x TEXT; THIS IS NOT SQL;";
        let mut conn = Connection::open(&path).unwrap();
        assert!(migrate_with(&mut conn, &path, &[V1, V2, V3, bad]).is_err());
        assert_eq!(version(&path), 3);
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
}
