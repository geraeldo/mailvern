use super::{Db, FolderSyncState, PendingOp, PendingOpPayload};
use crate::model::*;
use anyhow::Result;
use rusqlite::{params, OptionalExtension};

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
        notify: r.get(13)?,
        badge_mode: r.get(14)?,
    })
}

impl Db {
    // ---------- accounts ----------

    pub fn accounts(&self) -> Result<Vec<Account>> {
        let c = self.r();
        let mut st = c.prepare(
            "SELECT id, name, email, imap_host, imap_port, smtp_host, smtp_port, color, signature, position, label, aliases_json, reply_to, notify, badge_mode
             FROM accounts ORDER BY position, id",
        )?;
        let rows = st.query_map([], row_to_account)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn account(&self, id: i64) -> Result<Account> {
        let c = self.r();
        Ok(c.query_row(
            "SELECT id, name, email, imap_host, imap_port, smtp_host, smtp_port, color, signature, position, label, aliases_json, reply_to, notify, badge_mode
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
                "UPDATE accounts SET name=?, email=?, imap_host=?, imap_port=?, smtp_host=?, smtp_port=?, color=?, signature=?, label=?, aliases_json=?, reply_to=?, notify=?, badge_mode=? WHERE id=?",
                params![a.name, a.email, a.imap_host, a.imap_port, a.smtp_host, a.smtp_port, a.color, a.signature, a.label, aliases_json, a.reply_to, a.notify, a.badge_mode, a.id],
            )?;
            Ok(a.id)
        } else {
            let pos: i64 = c.query_row("SELECT COALESCE(MAX(position), 0) + 1 FROM accounts", [], |r| r.get(0))?;
            c.execute(
                "INSERT INTO accounts (name, email, imap_host, imap_port, smtp_host, smtp_port, color, signature, position, label, aliases_json, reply_to, notify, badge_mode)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![a.name, a.email, a.imap_host, a.imap_port, a.smtp_host, a.smtp_port, a.color, a.signature, pos, a.label, aliases_json, a.reply_to, a.notify, a.badge_mode],
            )?;
            Ok(c.last_insert_rowid())
        }
    }

    pub fn set_account_alerts(&self, account_id: i64, notify: bool, badge_mode: &str) -> Result<()> {
        let c = self.c();
        c.execute(
            "UPDATE accounts SET notify = ?, badge_mode = ? WHERE id = ?",
            params![notify, badge_mode, account_id],
        )?;
        Ok(())
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
        tx.execute("DELETE FROM moved_bodies WHERE account_id = ?", [id])?;
        tx.commit()?;
        let _ = c.execute_batch("PRAGMA incremental_vacuum(200);");
        Ok(())
    }

    // ---------- folders ----------

    pub fn upsert_folder(&self, account_id: i64, path: &str, name: &str, role: Option<&str>) -> Result<i64> {
        let c = self.c();
        c.prepare_cached(
            "INSERT INTO folders (account_id, path, name, role) VALUES (?, ?, ?, ?)
             ON CONFLICT(account_id, path) DO UPDATE SET name = excluded.name, role = excluded.role",
        )?
        .execute(params![account_id, path, name, role])?;
        let id = c
            .prepare_cached("SELECT id FROM folders WHERE account_id = ? AND path = ?")?
            .query_row(params![account_id, path], |r| r.get(0))?;
        Ok(id)
    }

    /// Upserts all folders of an account in a single SQLite transaction.
    pub fn upsert_folders_batch(&self, account_id: i64, items: &[(&str, &str, Option<&str>)]) -> Result<Vec<i64>> {
        let mut c = self.c();
        let tx = c.transaction()?;
        let mut out = Vec::with_capacity(items.len());
        {
            let mut ins = tx.prepare_cached(
                "INSERT INTO folders (account_id, path, name, role) VALUES (?, ?, ?, ?)
                 ON CONFLICT(account_id, path) DO UPDATE SET name = excluded.name, role = excluded.role",
            )?;
            let mut sel = tx.prepare_cached("SELECT id FROM folders WHERE account_id = ? AND path = ?")?;
            for &(path, name, role) in items {
                ins.execute(params![account_id, path, name, role])?;
                out.push(sel.query_row(params![account_id, path], |r| r.get(0))?);
            }
        }
        tx.commit()?;
        Ok(out)
    }

    /// Unread count of a single folder (avoids scanning every folder of every account).
    pub fn folder_unread(&self, folder_id: i64) -> Result<i64> {
        let c = self.r();
        let count = c
            .prepare_cached("SELECT COUNT(*) FROM messages WHERE folder_id = ? AND seen = 0")?
            .query_row([folder_id], |r| r.get(0))?;
        Ok(count)
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

}
