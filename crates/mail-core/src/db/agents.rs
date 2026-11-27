use super::Db;
use crate::model::*;
use anyhow::Result;
use rusqlite::{params, OptionalExtension};

impl Db {
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

    /// Account ids explicitly hidden from MCP agents.
    pub fn agent_disabled_accounts(&self) -> Result<Vec<i64>> {
        let c = self.r();
        let mut s = c.prepare("SELECT account_id FROM agent_account_access WHERE allowed = 0 ORDER BY account_id")?;
        let rows = s.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn set_agent_account_allowed(&self, account_id: i64, allowed: bool) -> Result<()> {
        let c = self.c();
        if allowed {
            c.execute("DELETE FROM agent_account_access WHERE account_id = ?", [account_id])?;
        } else {
            c.execute(
                "INSERT INTO agent_account_access (account_id, allowed) VALUES (?, 0)
                 ON CONFLICT(account_id) DO UPDATE SET allowed = 0",
                [account_id],
            )?;
        }
        Ok(())
    }

    /// Explicit per-folder MCP read visibility overrides (`folder_id -> allowed`).
    pub fn agent_folder_overrides(&self) -> Result<std::collections::HashMap<i64, bool>> {
        let c = self.r();
        let mut s = c.prepare("SELECT folder_id, allowed FROM agent_folder_access")?;
        let mut out = std::collections::HashMap::new();
        for r in s.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)? != 0)))? {
            let (id, allowed) = r?;
            out.insert(id, allowed);
        }
        Ok(out)
    }

    pub fn set_agent_folder_allowed(&self, folder_id: i64, allowed: Option<bool>) -> Result<()> {
        let c = self.c();
        match allowed {
            Some(v) => {
                c.execute(
                    "INSERT INTO agent_folder_access (folder_id, allowed) VALUES (?, ?)
                     ON CONFLICT(folder_id) DO UPDATE SET allowed = excluded.allowed",
                    params![folder_id, v as i64],
                )?;
            }
            None => {
                c.execute("DELETE FROM agent_folder_access WHERE folder_id = ?", [folder_id])?;
            }
        }
        Ok(())
    }

    /// Returns `(account_id, folder_id, role)` for the given message AND every other folder copy
    /// sharing its `(account_id, message_id)` (e.g. Spam/Trash vs Gmail's All Mail).
    pub fn message_folder_copies(&self, message_id: i64) -> Result<Vec<(i64, i64, Option<String>)>> {
        let c = self.r();
        let Some((account_id, folder_id, role, mid)): Option<(i64, i64, Option<String>, Option<String>)> = c
            .query_row(
                "SELECT m.account_id, m.folder_id, f.role, m.message_id
                 FROM messages m JOIN folders f ON f.id = m.folder_id WHERE m.id = ?",
                [message_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?
        else {
            return Ok(vec![]);
        };
        let Some(mid) = mid else {
            return Ok(vec![(account_id, folder_id, role)]);
        };
        let mut s = c.prepare(
            "SELECT DISTINCT m.account_id, m.folder_id, f.role
             FROM messages m JOIN folders f ON f.id = m.folder_id
             WHERE m.account_id = ?1 AND m.message_id = ?2",
        )?;
        let rows = s.query_map(params![account_id, mid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
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

}
