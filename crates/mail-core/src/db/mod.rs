mod agents;
mod folders;
mod messages;
mod migrations;
mod outbox;
mod rules;
mod search;
pub(crate) use search::normalize_fts;
#[cfg(test)]
mod tests;

pub use outbox::{DueSnooze, OutboxRow};

use crate::icons::Icon;
use crate::model::*;
use anyhow::{anyhow, Result};
use messages::{assign_thread, decompress_raw};
use migrations::{migrate, CACHES};
#[cfg(test)]
use migrations::{migrate_with, MIGRATIONS, SCHEMA_VERSION, V1, V2, V3, V4, V5, V6, V7};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

pub type SummaryDetailRow = (MessageSummary, Vec<Addr>, Vec<Addr>, Vec<Addr>, Option<String>, Option<String>, Option<String>, Option<String>);
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
#[derive(Clone)]
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
#[derive(Default)]
pub struct BodyInfo {
    pub snippet: String,
    pub search_text: String,
    pub has_attachments: bool,
    pub attachment_names: String,
}

/// Per-connection settings. Not part of the schema; applied on every open.
const PRAGMAS: &str = "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000; PRAGMA temp_store = MEMORY; PRAGMA cache_size = -8000;";
const READ_PRAGMAS: &str = "PRAGMA busy_timeout = 5000; PRAGMA query_only = ON; PRAGMA temp_store = MEMORY; PRAGMA cache_size = -4000;";


const SUMMARY_COLS: &str =
    "m.id, m.account_id, m.folder_id, m.from_name, m.from_addr, m.subject, m.snippet, m.date, m.seen, m.flagged, m.has_attachments,
     m.thread_id,
     (SELECT s.wake_at FROM snoozes s WHERE s.account_id = m.account_id AND s.message_id = m.message_id AND s.woke_at IS NULL)";

/// Number of columns in SUMMARY_COLS.
const SUMMARY_LEN: usize = 13;

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
        read.execute_batch(READ_PRAGMAS)?;
        let db = Db { conn: Mutex::new(conn), read: Mutex::new(read) };
        db.assign_threads()?;
        db.repair_html_snippets()?;
        Ok(db)
    }

    /// One-time repair for cached messages whose stored snippet contains raw HTML tags
    /// (e.g., from senders that placed HTML inside a `text/plain` part).
    fn repair_html_snippets(&self) -> Result<()> {
        const KEY: &str = "snippets_html_repaired_v1";
        let already_done: bool = self
            .r()
            .query_row("SELECT EXISTS(SELECT 1 FROM settings WHERE key = ?)", [KEY], |r| r.get(0))?;
        if already_done {
            return Ok(());
        }
        let candidates: Vec<(i64, String, Vec<u8>)> = {
            let r = self.r();
            let mut st = r.prepare(
                "SELECT m.id, m.snippet, CASE WHEN b.blob_id IS NOT NULL THEN rb.raw ELSE b.raw END
                 FROM messages m
                 JOIN bodies b ON b.message_id = m.id
                 LEFT JOIN raw_blobs rb ON rb.id = b.blob_id
                 WHERE m.has_body = 1 AND m.snippet LIKE '%<%'",
            )?;
            let rows = st
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect::<rusqlite::Result<_>>()?;
            rows
        };
        let updates: Vec<(i64, String, String)> = candidates
            .into_iter()
            .filter_map(|(id, old_snippet, stored_raw)| {
                let raw = decompress_raw(&stored_raw);
                let info = crate::parse::body_info(&raw);
                if info.snippet != old_snippet {
                    Some((id, info.snippet, info.search_text))
                } else {
                    None
                }
            })
            .collect();
        let mut c = self.c();
        let tx = c.transaction()?;
        if !updates.is_empty() {
            let mut upd_msg = tx.prepare_cached("UPDATE messages SET snippet = ? WHERE id = ?")?;
            let mut upd_fts = tx.prepare_cached("UPDATE messages_fts SET body = ? WHERE rowid = ?")?;
            for (id, snippet, search_text) in updates {
                upd_msg.execute(params![snippet, id])?;
                upd_fts.execute(params![search::normalize_fts(&search_text), id])?;
            }
        }
        tx.execute(
            "INSERT INTO settings (key, value) VALUES (?, '1') ON CONFLICT(key) DO UPDATE SET value = '1'",
            [KEY],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Reclaims freed pages (when incremental auto_vacuum is active), updates SQLite planner
    /// statistics, and performs a non-blocking WAL checkpoint.
    pub fn maintenance(&self) {
        let c = self.c();
        let _ = c.execute_batch("PRAGMA incremental_vacuum(200); PRAGMA optimize; PRAGMA wal_checkpoint(PASSIVE);");
    }

    /// Gives every message without a conversation one, oldest first (needed once after v4).
    fn assign_threads(&self) -> Result<()> {
        // Fast probe using partial index `idx_messages_no_thread` (0 bytes / ~0ms when up to date).
        let has_unassigned: bool = self
            .r()
            .query_row("SELECT EXISTS(SELECT 1 FROM messages WHERE thread_id IS NULL)", [], |r| r.get(0))?;
        if !has_unassigned {
            return Ok(());
        }
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


    // ---------- settings ----------

    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        let c = self.r();
        let v = c.prepare_cached("SELECT value FROM settings WHERE key = ?")?.query_row([key], |r| r.get(0)).optional()?;
        Ok(v)
    }

    /// Loads multiple settings in a single SQLite read.
    pub fn settings_many(&self, keys: &[&str]) -> Result<HashMap<String, String>> {
        if keys.is_empty() {
            return Ok(HashMap::new());
        }
        let c = self.r();
        let json = serde_json::to_string(keys)?;
        let mut st = c.prepare_cached("SELECT s.key, s.value FROM settings s JOIN json_each(?1) j ON s.key = j.value")?;
        let rows = st.query_map([json], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        Ok(rows)
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
        const MAX_ICON_DOMAINS: i64 = 1000;
        const MAX_ICON_BYTES: i64 = 10 * 1024 * 1024;
        let c = self.c();
        c.prepare_cached(
            "INSERT INTO sender_icons (domain, mime, data, logo, retry_at) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(domain) DO UPDATE SET mime = excluded.mime, data = excluded.data, logo = excluded.logo, retry_at = excluded.retry_at",
        )?
        .execute(params![domain, icon.map(|i| &i.mime), icon.map(|i| &i.data), icon.is_some_and(|i| i.logo), retry_at])?;
        let (count, total_bytes): (i64, i64) = c
            .prepare_cached("SELECT COUNT(*), COALESCE(SUM(LENGTH(data)), 0) FROM sender_icons")?
            .query_row([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        if count > MAX_ICON_DOMAINS || total_bytes > MAX_ICON_BYTES {
            let keep = if total_bytes > MAX_ICON_BYTES { (count * 7) / 10 } else { MAX_ICON_DOMAINS };
            let _ = c.execute(
                "DELETE FROM sender_icons WHERE domain NOT IN (SELECT domain FROM sender_icons ORDER BY retry_at DESC LIMIT ?)",
                [keep.max(100)],
            );
        }
        Ok(())
    }
}
