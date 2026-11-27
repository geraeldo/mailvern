use super::Db;
use crate::model::*;
use anyhow::{anyhow, Result};
use rusqlite::{params, OptionalExtension};

impl Db {
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
        if ids.is_empty() {
            return Ok(());
        }
        let mut c = self.c();
        let tx = c.transaction()?;
        {
            let mut st = tx.prepare_cached("DELETE FROM rule_queue WHERE message_id = ?")?;
            for id in ids {
                st.execute([id])?;
            }
        }
        tx.commit()?;
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

    /// The body as plain text (quoted replies left out), if it has been downloaded (including evicted rows whose FTS text was kept).
    pub fn body_text(&self, id: i64) -> Result<Option<String>> {
        let c = self.r();
        let state: Option<i64> = c.prepare_cached("SELECT has_body FROM messages WHERE id = ?")?.query_row([id], |r| r.get(0)).optional()?;
        if !matches!(state, Some(1) | Some(2)) {
            return Ok(None);
        }
        let body = c.prepare_cached("SELECT body FROM messages_fts WHERE rowid = ?")?.query_row([id], |r| r.get(0)).optional()?.unwrap_or_default();
        Ok(Some(body))
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
        c.prepare_cached(
            "INSERT INTO rule_log (rule_id, rule_name, account_id, msgid, subject, from_addr, action, at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )?
        .execute(params![rule.id, rule.name, account_id, msgid, subject, from_addr, serde_json::to_string(action)?, now])?;
        let id = c.last_insert_rowid();
        // Keep the log from growing forever without running a prune scan on every single insert.
        if id % 100 == 0 {
            let _ = c.execute("DELETE FROM rule_log WHERE id NOT IN (SELECT id FROM rule_log ORDER BY id DESC LIMIT 2000)", []);
        }
        Ok(id)
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

}
