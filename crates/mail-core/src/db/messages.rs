use super::search::{fts_query, like_any, normalize_fts};
use super::{row_to_summary, scope_sql, BodyInfo, Db, NewMessage, SummaryDetailRow, SUMMARY_COLS, SUMMARY_LEN};
use crate::model::*;
use anyhow::{anyhow, Result};
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;
use std::io::{Read, Write};

/// Magic header for zlib-compressed raw RFC 822 blobs stored in SQLite.
/// Valid RFC 822 headers are ASCII and never start with a NUL byte, so legacy
/// uncompressed blobs decode transparently without migration downtime.
const ZLIB_MAGIC: &[u8; 4] = b"\x00MVZ";

pub(super) fn compress_raw(raw: &[u8]) -> Vec<u8> {
    if raw.len() < 512 {
        return raw.to_vec();
    }
    let mut enc = ZlibEncoder::new(Vec::with_capacity(raw.len() / 3 + 16), Compression::fast());
    if enc.write_all(raw).is_err() {
        return raw.to_vec();
    }
    let Ok(compressed) = enc.finish() else {
        return raw.to_vec();
    };
    if compressed.len() + ZLIB_MAGIC.len() < raw.len() {
        let mut out = Vec::with_capacity(ZLIB_MAGIC.len() + compressed.len());
        out.extend_from_slice(ZLIB_MAGIC);
        out.extend_from_slice(&compressed);
        out
    } else {
        raw.to_vec()
    }
}

pub(super) fn decompress_raw(stored: &[u8]) -> Vec<u8> {
    if let Some(payload) = stored.strip_prefix(ZLIB_MAGIC) {
        let mut dec = ZlibDecoder::new(payload);
        let mut out = Vec::with_capacity(payload.len().saturating_mul(3));
        if dec.read_to_end(&mut out).is_ok() {
            return out;
        }
    }
    stored.to_vec()
}


/// Message-ids in a References / In-Reply-To value, in order: "<a> <b>" -> ["a", "b"].
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
pub(super) fn assign_thread(
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

impl Db {
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
        if uids.is_empty() {
            return Ok(vec![]);
        }
        let c = self.r();
        let json = serde_json::to_string(uids)?;
        let mut st = c.prepare_cached(
            "SELECT m.id, m.uid FROM messages m JOIN json_each(?2) j ON m.uid = j.value WHERE m.folder_id = ?1",
        )?;
        let by_uid: HashMap<u32, i64> = st
            .query_map(params![folder_id, json], |r| Ok((r.get(1)?, r.get(0)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(uids.iter().filter_map(|u| by_uid.get(u).copied()).collect())
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
            let is_sent: bool = tx
                .prepare_cached("SELECT role = 'sent' FROM folders WHERE id = ?")?
                .query_row([folder_id], |r| r.get::<_, Option<bool>>(0))
                .optional()?
                .flatten()
                .unwrap_or(false);
            let mut ins = tx.prepare_cached(
                "INSERT OR IGNORE INTO messages (account_id, folder_id, uid, message_id, in_reply_to, refs, subject,
                    from_name, from_addr, to_json, cc_json, reply_to_json, date, sort_date, seen, flagged, has_attachments, size, sparse,
                    list_id, auto)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )?;
            // Sync reaching a stored search hit takes it into the window.
            let mut unsparse = tx.prepare_cached("UPDATE messages SET sparse = 0 WHERE folder_id = ? AND uid = ? AND sparse = 1")?;
            let mut fts = tx.prepare_cached("INSERT INTO messages_fts (rowid, subject, sender, body) VALUES (?, ?, ?, '')")?;
            let mut bump_contact = tx.prepare_cached(
                "INSERT INTO contacts_cache (email_lc, email, name, score) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(email_lc) DO UPDATE SET
                   score = contacts_cache.score + excluded.score,
                   name = CASE WHEN contacts_cache.name = '' AND excluded.name != '' THEN excluded.name ELSE contacts_cache.name END",
            )?;
            let mut snooze_woke = tx.prepare_cached(
                "SELECT woke_at FROM snoozes WHERE account_id = ? AND message_id = ? AND woke_at IS NOT NULL",
            )?;
            for m in msgs {
                let woke: i64 = match m.message_id.as_deref() {
                    Some(mid) => snooze_woke
                        .query_row(params![account_id, mid], |r| r.get::<_, i64>(0))
                        .optional()?
                        .unwrap_or(0),
                    None => 0,
                };
                let sort_date = m.date.max(woke);
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
                    sort_date,
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
                    fts.execute(params![id, normalize_fts(&m.subject), normalize_fts(&format!("{} {}", m.from_name, m.from_addr))])?;
                    assign_thread(&tx, account_id, id, m.message_id.as_deref(), m.in_reply_to.as_deref(), m.refs.as_deref())?;
                    if !m.from_addr.is_empty() {
                        let _ = bump_contact.execute(params![m.from_addr.to_lowercase(), m.from_addr, m.from_name, 1i64]);
                    }
                    if is_sent {
                        for rcpt in &m.to {
                            if !rcpt.email.is_empty() {
                                let _ = bump_contact.execute(params![
                                    rcpt.email.to_lowercase(),
                                    rcpt.email,
                                    rcpt.name.as_deref().unwrap_or(""),
                                    4i64
                                ]);
                            }
                        }
                    }
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
            let mut del_msg = tx.prepare_cached("DELETE FROM messages WHERE id = ?")?;
            let mut upd_msg = tx.prepare_cached("UPDATE messages SET seen = ?, flagged = ? WHERE id = ?")?;
            for (id, uid, seen, flagged) in local {
                local_uids.insert(uid);
                if skip_uids.contains(&uid) {
                    continue;
                }
                match server.get(&uid) {
                    None => {
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
            let mut del_msg = tx.prepare_cached("DELETE FROM messages WHERE id = ?")?;
            for (id, uid) in local {
                local_uids.insert(uid);
                if skip_uids.contains(&uid) {
                    continue;
                }
                if !server_uids.contains(&uid) {
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
                    tx.prepare_cached("DELETE FROM messages WHERE id = ?")?.execute([id])?;
                    changed = true;
                }
                Some(&(s, f)) if s != seen || f != flagged => {
                    tx.prepare_cached("UPDATE messages SET seen = ?, flagged = ? WHERE id = ?")?.execute(params![s, f, id])?;
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

    fn upsert_blob_in_tx(tx: &rusqlite::Transaction<'_>, account_id: i64, msg_id: i64, message_id: Option<&str>, compressed: &[u8]) -> rusqlite::Result<()> {
        if let Some(mid) = message_id.filter(|s| !s.is_empty()) {
            tx.prepare_cached(
                "INSERT INTO raw_blobs (account_id, message_id, raw) VALUES (?1, ?2, ?3)
                 ON CONFLICT(account_id, message_id) DO UPDATE SET raw = excluded.raw",
            )?
            .execute(params![account_id, mid, compressed])?;
            let blob_id: i64 = tx
                .prepare_cached("SELECT id FROM raw_blobs WHERE account_id = ?1 AND message_id = ?2")?
                .query_row(params![account_id, mid], |r| r.get(0))?;
            tx.prepare_cached("INSERT OR REPLACE INTO bodies (message_id, raw, blob_id) VALUES (?1, X'', ?2)")?
                .execute(params![msg_id, blob_id])?;
        } else {
            tx.prepare_cached("INSERT OR REPLACE INTO bodies (message_id, raw, blob_id) VALUES (?1, ?2, NULL)")?
                .execute(params![msg_id, compressed])?;
        }
        Ok(())
    }

    pub fn store_body(&self, folder_id: i64, uid: u32, raw: &[u8], info: &BodyInfo) -> Result<()> {
        let compressed = compress_raw(raw);
        let mut c = self.c();
        let tx = c.transaction()?;
        let row: Option<(i64, i64, Option<String>)> = tx
            .prepare_cached("SELECT id, account_id, message_id FROM messages WHERE folder_id = ? AND uid = ?")?
            .query_row(params![folder_id, uid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .optional()?;
        if let Some((id, account_id, message_id)) = row {
            Self::upsert_blob_in_tx(&tx, account_id, id, message_id.as_deref(), &compressed)?;
            tx.prepare_cached("UPDATE messages SET has_body = 1, snippet = ?, has_attachments = ?, attachment_names = ? WHERE id = ?")?
                .execute(params![info.snippet, info.has_attachments, info.attachment_names, id])?;
            tx.prepare_cached("UPDATE messages_fts SET body = ? WHERE rowid = ?")?
                .execute(params![normalize_fts(&info.search_text), id])?;
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
        // Compress outside the SQLite write lock so UI queries aren't delayed.
        let prepared: Vec<(u32, Option<Vec<u8>>, &BodyInfo)> = items
            .iter()
            .map(|(uid, raw, info)| {
                let comp = if info.has_attachments { None } else { Some(compress_raw(raw)) };
                (*uid, comp, info)
            })
            .collect();
        let mut c = self.c();
        let tx = c.transaction()?;
        let mut stored = 0;
        {
            let mut find_id = tx.prepare_cached("SELECT id, account_id, message_id FROM messages WHERE folder_id = ? AND uid = ?")?;
            let mut upd_msg = tx.prepare_cached("UPDATE messages SET has_body = 1, snippet = ?, has_attachments = 0 WHERE id = ?")?;
            let mut mark_att = tx.prepare_cached("UPDATE messages SET has_attachments = 1, attachment_names = ? WHERE id = ?")?;
            let mut upd_fts = tx.prepare_cached("UPDATE messages_fts SET body = ? WHERE rowid = ?")?;
            for (uid, comp, info) in prepared {
                let row: Option<(i64, i64, Option<String>)> = find_id
                    .query_row(params![folder_id, uid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                    .optional()?;
                if let Some((id, account_id, message_id)) = row {
                    let Some(compressed) = comp else {
                        mark_att.execute(params![info.attachment_names, id])?;
                        continue;
                    };
                    Self::upsert_blob_in_tx(&tx, account_id, id, message_id.as_deref(), &compressed)?;
                    upd_msg.execute(params![info.snippet, id])?;
                    upd_fts.execute(params![normalize_fts(&info.search_text), id])?;
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
            let mut sel = tx.prepare_cached(
                "SELECT m.account_id, m.message_id, CASE WHEN b.blob_id IS NOT NULL THEN rb.raw ELSE b.raw END,
                        m.snippet, COALESCE(f.body, ''), m.has_attachments
                 FROM messages m
                 JOIN bodies b ON b.message_id = m.id
                 LEFT JOIN raw_blobs rb ON rb.id = b.blob_id
                 LEFT JOIN messages_fts f ON f.rowid = m.id
                 WHERE m.id = ? AND m.message_id IS NOT NULL AND m.has_body = 1",
            )?;
            let mut ins = tx.prepare_cached(
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
    /// Shares the compressed `raw_blobs` row (zero blob duplication across folders).
    pub fn copy_existing_bodies_for_folder(&self, account_id: i64, folder_id: i64) -> Result<usize> {
        let mut c = self.c();
        let tx = c.transaction()?;
        let mut hydrated = 0;
        {
            let candidates: Vec<(i64, String)> = tx
                .prepare_cached("SELECT id, message_id FROM messages WHERE folder_id = ? AND has_body = 0 AND message_id IS NOT NULL")?
                .query_map([folder_id], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            if !candidates.is_empty() {
                let mut from_stash = tx.prepare_cached(
                    "SELECT raw, snippet, search_text, has_attachments FROM moved_bodies WHERE account_id = ? AND message_id = ?",
                )?;
                let mut del_stash = tx.prepare_cached("DELETE FROM moved_bodies WHERE account_id = ? AND message_id = ?")?;
                let mut from_other = tx.prepare_cached(
                    "SELECT b.blob_id, b.raw, m.snippet, COALESCE(f.body, ''), m.has_attachments
                     FROM messages m
                     JOIN bodies b ON b.message_id = m.id
                     LEFT JOIN messages_fts f ON f.rowid = m.id
                     WHERE m.account_id = ? AND m.message_id = ? AND m.has_body = 1 AND m.id != ?
                     LIMIT 1",
                )?;
                let mut link_blob = tx.prepare_cached("INSERT OR REPLACE INTO bodies (message_id, raw, blob_id) VALUES (?, X'', ?)")?;
                let mut upd_msg = tx.prepare_cached("UPDATE messages SET has_body = 1, snippet = ?, has_attachments = ? WHERE id = ?")?;
                let mut upd_fts = tx.prepare_cached("UPDATE messages_fts SET body = ? WHERE rowid = ?")?;

                for (id, mid) in candidates {
                    let stashed: Option<(Vec<u8>, String, String, bool)> = from_stash
                        .query_row(params![account_id, &mid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
                        .optional()?;
                    if let Some((stored_raw, snippet, search_text, has_att)) = stashed {
                        del_stash.execute(params![account_id, &mid])?;
                        let blob_bytes = if stored_raw.starts_with(ZLIB_MAGIC) { stored_raw } else { compress_raw(&stored_raw) };
                        Self::upsert_blob_in_tx(&tx, account_id, id, Some(&mid), &blob_bytes)?;
                        upd_msg.execute(params![snippet, has_att, id])?;
                        upd_fts.execute(params![normalize_fts(&search_text), id])?;
                        hydrated += 1;
                        continue;
                    }
                    let other: Option<(Option<i64>, Vec<u8>, String, String, bool)> = from_other
                        .query_row(params![account_id, &mid, id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
                        .optional()?;
                    if let Some((blob_id, legacy_raw, snippet, search_text, has_att)) = other {
                        if let Some(bid) = blob_id {
                            // Zero-copy share of the deduplicated compressed blob!
                            link_blob.execute(params![id, bid])?;
                        } else {
                            let blob_bytes = if legacy_raw.starts_with(ZLIB_MAGIC) { legacy_raw } else { compress_raw(&legacy_raw) };
                            Self::upsert_blob_in_tx(&tx, account_id, id, Some(&mid), &blob_bytes)?;
                        }
                        upd_msg.execute(params![snippet, has_att, id])?;
                        upd_fts.execute(params![normalize_fts(&search_text), id])?;
                        hydrated += 1;
                    }
                }
            }
        }
        tx.commit()?;
        Ok(hydrated)
    }

    /// Evicts oldest cached raw bodies for `account_id` when total stored body size exceeds `max_bytes`.
    /// Marks evicted rows `has_body = 2` (NOT `0`) so background prefetch never re-downloads them,
    /// while preserving snippets and FTS body text and keeping on-demand opening working.
    pub fn prune_old_bodies(&self, account_id: i64, max_bytes: i64) -> Result<usize> {
        let total: i64 = {
            let r = self.r();
            let shared: i64 = r
                .prepare_cached("SELECT COALESCE(SUM(LENGTH(raw)), 0) FROM raw_blobs WHERE account_id = ?")?
                .query_row([account_id], |row| row.get(0))?;
            let direct: i64 = r
                .prepare_cached(
                    "SELECT COALESCE(SUM(LENGTH(b.raw)), 0) FROM bodies b JOIN messages m ON m.id = b.message_id WHERE m.account_id = ? AND b.blob_id IS NULL",
                )?
                .query_row([account_id], |row| row.get(0))?;
            shared + direct
        };
        if total <= max_bytes {
            // Still GC any orphaned raw_blobs left behind by message deletes.
            let c = self.c();
            let _ = c.execute(
                "DELETE FROM raw_blobs WHERE account_id = ? AND id NOT IN (SELECT blob_id FROM bodies WHERE blob_id IS NOT NULL)",
                [account_id],
            );
            return Ok(0);
        }
        let target = (max_bytes * 8) / 10;
        let mut to_drop = total - target;
        let mut c = self.c();
        let tx = c.transaction()?;
        let mut evicted = 0;
        {
            let candidates: Vec<(i64, Option<i64>, i64)> = tx
                .prepare_cached(
                    "SELECT m.id, b.blob_id, CASE WHEN b.blob_id IS NOT NULL THEN LENGTH(rb.raw) ELSE LENGTH(b.raw) END
                     FROM bodies b
                     JOIN messages m ON m.id = b.message_id
                     JOIN folders f ON f.id = m.folder_id
                     LEFT JOIN raw_blobs rb ON rb.id = b.blob_id
                     WHERE m.account_id = ?
                     ORDER BY (f.role IS 'inbox') ASC, m.date ASC, m.id ASC",
                )?
                .query_map([account_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<rusqlite::Result<_>>()?;
            let mut del_body = tx.prepare_cached("DELETE FROM bodies WHERE message_id = ?")?;
            let mut del_blob_bodies = tx.prepare_cached("DELETE FROM bodies WHERE blob_id = ?")?;
            let mut mark_evicted = tx.prepare_cached("UPDATE messages SET has_body = 2 WHERE id = ?")?;
            let mut mark_blob_evicted = tx.prepare_cached(
                "UPDATE messages SET has_body = 2 WHERE id IN (SELECT message_id FROM bodies WHERE blob_id = ?)",
            )?;
            let mut dropped_blobs = std::collections::HashSet::new();
            for (id, blob_id, sz) in candidates {
                if to_drop <= 0 {
                    break;
                }
                if let Some(bid) = blob_id {
                    if !dropped_blobs.insert(bid) {
                        continue;
                    }
                    mark_blob_evicted.execute([bid])?;
                    del_blob_bodies.execute([bid])?;
                } else {
                    del_body.execute([id])?;
                    mark_evicted.execute([id])?;
                }
                to_drop -= sz;
                evicted += 1;
            }
            tx.execute(
                "DELETE FROM raw_blobs WHERE account_id = ? AND id NOT IN (SELECT blob_id FROM bodies WHERE blob_id IS NOT NULL)",
                [account_id],
            )?;
        }
        tx.commit()?;
        let _ = c.execute_batch("PRAGMA incremental_vacuum(200);");
        Ok(evicted)
    }

    pub fn body(&self, id: i64) -> Result<Option<Vec<u8>>> {
        let stored: Option<Vec<u8>> = {
            let c = self.r();
            let mut st = c.prepare_cached(
                "SELECT CASE WHEN b.blob_id IS NOT NULL THEN rb.raw ELSE b.raw END
                 FROM bodies b
                 LEFT JOIN raw_blobs rb ON rb.id = b.blob_id
                 WHERE b.message_id = ?",
            )?;
            st.query_row([id], |r| r.get(0)).optional()?
        };
        Ok(stored.map(|b| decompress_raw(&b)))
    }

    /// (account_id, folder_id, folder_path, uid)
    pub fn locate(&self, id: i64) -> Result<(i64, i64, String, u32)> {
        let c = self.r();
        let row = c
            .prepare_cached(
                "SELECT m.account_id, m.folder_id, f.path, m.uid FROM messages m JOIN folders f ON f.id = m.folder_id WHERE m.id = ?",
            )?
            .query_row([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
        Ok(row)
    }

    /// Batched `(id, account_id, folder_id, uid)` lookup for bulk message actions.
    pub fn locate_many(&self, ids: &[i64]) -> Result<Vec<(i64, i64, i64, u32)>> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let c = self.r();
        let json = serde_json::to_string(ids)?;
        let mut st = c.prepare_cached(
            "SELECT m.id, m.account_id, m.folder_id, m.uid
             FROM messages m
             JOIN json_each(?1) j ON m.id = j.value",
        )?;
        let by_id: HashMap<i64, (i64, i64, u32)> = st
            .query_map([json], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?, r.get(3)?))))?
            .collect::<rusqlite::Result<_>>()?;
        let mut out = Vec::with_capacity(ids.len());
        for &id in ids {
            let &(acc, folder_id, uid) = by_id.get(&id).ok_or_else(|| anyhow!("Query returned no rows"))?;
            out.push((id, acc, folder_id, uid));
        }
        Ok(out)
    }

    /// One row per conversation: its newest message in the view, standing for all of its
    /// messages in the view (`ids`). Conversations are ordered by their freshest message.
    /// Uses SQLite's single-`MAX()` bare-column guarantee with `GROUP BY` (3x faster than
    /// window functions), denormalized `sort_date`, and runs correlated snooze / cross-folder
    /// count lookups ONLY for the returned page.
    pub fn list_threads(&self, view: &View, offset: i64, limit: i64) -> Result<Vec<MessageSummary>> {
        let c = self.r();
        let scope = scope_sql(view)?;
        let sql = format!(
            "WITH grouped AS (
               SELECT m.id, m.account_id, m.folder_id, m.from_name, m.from_addr, m.subject, m.snippet,
                      m.date, m.message_id, m.thread_id,
                      CASE WHEN SUM(CASE WHEN m.seen = 0 THEN 1 ELSE 0 END) = 0 THEN 1 ELSE 0 END AS t_seen,
                      CASE WHEN SUM(m.flagged) > 0 THEN 1 ELSE 0 END AS t_flagged,
                      CASE WHEN SUM(m.has_attachments) > 0 THEN 1 ELSE 0 END AS t_att,
                      GROUP_CONCAT(m.id) AS t_ids,
                      MAX(MIN(9000000000, MAX(-9000000000, m.sort_date)) * 1000000000 + m.id) AS t_sort
               FROM messages m WHERE {scope}
               GROUP BY m.account_id, COALESCE(m.thread_id, 'm' || m.id)
               ORDER BY t_sort DESC LIMIT ?1 OFFSET ?2
             )
             SELECT m.id, m.account_id, m.folder_id, m.from_name, m.from_addr, m.subject, m.snippet, m.date,
                    m.t_seen, m.t_flagged, m.t_att, m.thread_id,
                    (SELECT s.wake_at FROM snoozes s WHERE s.account_id = m.account_id AND s.message_id = m.message_id AND s.woke_at IS NULL),
                    m.t_ids,
                    (SELECT COUNT(DISTINCT COALESCE(x.message_id, x.id)) FROM messages x
                     WHERE x.account_id = m.account_id AND x.thread_id = m.thread_id
                       AND x.folder_id NOT IN (SELECT id FROM folders WHERE role IN ('trash', 'junk')))
             FROM grouped m
             ORDER BY m.t_sort DESC"
        );
        let mut st = c.prepare_cached(&sql)?;
        let rows = st
            .query_map(params![limit, offset], |r| {
                let mut s = row_to_summary(r)?;
                let ids: String = r.get(SUMMARY_LEN)?;
                s.ids = ids.split(',').filter_map(|i| i.parse().ok()).collect();
                let total: i64 = r.get(SUMMARY_LEN + 1)?;
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
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let c = self.r();
        let json = serde_json::to_string(ids)?;
        let sql = format!("SELECT {SUMMARY_COLS} FROM messages m JOIN json_each(?1) j ON m.id = j.value");
        let mut st = c.prepare_cached(&sql)?;
        let by_id: HashMap<i64, MessageSummary> = st
            .query_map([json], |r| {
                let s = row_to_summary(r)?;
                Ok((s.id, s))
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(ids.iter().filter_map(|id| by_id.get(id).cloned()).collect())
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
                "SELECT {SUMMARY_COLS} FROM messages m WHERE {scope} AND ?1 IS NULL ORDER BY m.sort_date DESC, m.id DESC LIMIT ?2 OFFSET ?3"
            ),
        };
        let mut st = c.prepare_cached(&sql)?;
        let rows = st
            .query_map(params![fts, limit, offset], row_to_summary)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn summary(&self, id: i64) -> Result<SummaryDetailRow> {
        let c = self.r();
        let sql = format!(
            "SELECT {SUMMARY_COLS}, m.to_json, m.cc_json, m.reply_to_json, m.message_id, m.refs, m.in_reply_to, m.list_id FROM messages m WHERE m.id = ?"
        );
        let row = c.prepare_cached(&sql)?.query_row([id], |r| {
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
                r.get(n + 6)?,
            ))
        })?;
        Ok(row)
    }

    pub fn set_seen(&self, ids: &[i64], seen: bool) -> Result<()> {
        let mut c = self.c();
        let tx = c.transaction()?;
        {
            let mut st = tx.prepare_cached("UPDATE messages SET seen = ? WHERE id = ?")?;
            for id in ids {
                st.execute(params![seen, id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Marks every cached message in a folder as read.
    pub fn mark_folder_read(&self, folder_id: i64) -> Result<usize> {
        let c = self.c();
        let n = c.prepare_cached("UPDATE messages SET seen = 1 WHERE folder_id = ? AND seen = 0")?.execute([folder_id])?;
        Ok(n)
    }

    pub fn set_flagged(&self, ids: &[i64], flagged: bool) -> Result<()> {
        let mut c = self.c();
        let tx = c.transaction()?;
        {
            let mut st = tx.prepare_cached("UPDATE messages SET flagged = ? WHERE id = ?")?;
            for id in ids {
                st.execute(params![flagged, id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn delete_messages(&self, ids: &[i64]) -> Result<()> {
        let mut c = self.c();
        let tx = c.transaction()?;
        {
            let mut st = tx.prepare_cached("DELETE FROM messages WHERE id = ?")?;
            for id in ids {
                st.execute([id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn mark_quiet(&self, id: i64) -> Result<()> {
        self.c().prepare_cached("UPDATE messages SET quiet = 1 WHERE id = ?")?.execute([id])?;
        Ok(())
    }

    /// The Message-ID header of a stored message.
    pub fn message_id_of(&self, id: i64) -> Result<Option<String>> {
        let c = self.r();
        let mid = c.prepare_cached("SELECT message_id FROM messages WHERE id = ?")?.query_row([id], |r| r.get(0))?;
        Ok(mid)
    }

    /// Removes local copies of a message from a folder (by Message-ID).
    pub fn delete_by_message_id(&self, folder_id: i64, message_id: &str) -> Result<usize> {
        let c = self.c();
        let n = c.prepare_cached("DELETE FROM messages WHERE folder_id = ? AND message_id = ?")?.execute(params![folder_id, message_id])?;
        Ok(n)
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

    #[cfg(test)]
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
    #[cfg(test)]
    pub fn unified_unread_after(&self, after_id: i64) -> Result<i64> {
        let c = self.r();
        Ok(c.query_row(
            "SELECT COUNT(*) FROM messages WHERE seen = 0 AND quiet = 0 AND id > ?1 AND folder_id IN (SELECT id FROM folders WHERE role = 'inbox')",
            [after_id],
            |r| r.get(0),
        )?)
    }

    pub fn account_inbox_unread(&self, account_id: i64) -> Result<i64> {
        let c = self.r();
        Ok(c.query_row(
            "SELECT COUNT(*) FROM messages WHERE account_id = ?1 AND seen = 0 AND folder_id IN (SELECT id FROM folders WHERE account_id = ?1 AND role = 'inbox')",
            [account_id],
            |r| r.get(0),
        )?)
    }

    pub fn account_inbox_unread_after(&self, account_id: i64, after_id: i64) -> Result<i64> {
        let c = self.r();
        Ok(c.query_row(
            "SELECT COUNT(*) FROM messages WHERE account_id = ?1 AND seen = 0 AND quiet = 0 AND id > ?2 AND folder_id IN (SELECT id FROM folders WHERE account_id = ?1 AND role = 'inbox')",
            params![account_id, after_id],
            |r| r.get(0),
        )?)
    }

    /// Address suggestions, ranked by how often you write to / hear from them.
    /// Reads the incrementally maintained `contacts_cache` instead of scanning `messages`.
    pub fn contacts(&self, q: &str) -> Result<Vec<Addr>> {
        let c = self.r();
        let like = like_any(q);
        let mut st = c.prepare_cached(
            "SELECT name, email FROM contacts_cache
             WHERE email LIKE ?1 ESCAPE '\\' OR name LIKE ?1 ESCAPE '\\'
             ORDER BY score DESC LIMIT 8",
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

}
