//! Agent permission resolution and pending/recent agent actions queue on `Engine`.

use super::{now, Engine};
use crate::model::*;
use anyhow::{anyhow, Result};

/// Settings key for the global default MCP permission mode ("approval" | "auto").
pub const AGENT_MODE_KEY: &str = "mcp_mode";
/// Maximum resolved (non-pending) agent action rows retained for the activity log.
const MAX_RESOLVED_HISTORY: usize = 200;

impl Engine {
    /// Current global default + per-account MCP permission overrides.
    pub fn agent_permissions(&self) -> Result<AgentPermissions> {
        let default_mode = self
            .db
            .setting(AGENT_MODE_KEY)?
            .as_deref()
            .map(AgentMode::parse)
            .unwrap_or_default();
        let account_modes = self.db.agent_account_modes()?;
        Ok(AgentPermissions { default_mode, account_modes })
    }

    /// Sets the global default MCP permission mode.
    pub fn set_agent_default_mode(&self, mode: AgentMode) -> Result<()> {
        self.db.set_setting(AGENT_MODE_KEY, mode.as_str())
    }

    /// Sets or clears (`None` = follow default) a per-account MCP permission override.
    pub fn set_agent_account_mode(&self, account_id: i64, mode: Option<AgentMode>) -> Result<()> {
        // Ensure the account exists before inserting a foreign-key row.
        self.db.account(account_id)?;
        self.db.set_agent_account_mode(account_id, mode)
    }

    fn emit_agent_actions_changed(&self) {
        let pending = self.db.pending_agent_actions_count().unwrap_or(0);
        let _ = self.events.send(Event::AgentActionsChanged { pending });
    }

    /// Emits an event so the UI can show an Undo toast when an agent sends mail in Auto mode.
    pub fn notify_agent_sent(&self, outbox_id: i64, account_id: i64, subject: String, to: Vec<String>) {
        let _ = self.events.send(Event::AgentSent { outbox_id, account_id, subject, to });
    }

    /// Adds a validated agent action to the pending approval queue and notifies the UI.
    pub fn queue_agent_action(
        &self,
        account_id: Option<i64>,
        spec: AgentActionSpec,
        summary: String,
        preview: AgentActionPreview,
    ) -> Result<AgentAction> {
        let payload = serde_json::to_string(&spec)?;
        let preview_json = serde_json::to_string(&preview)?;
        let id = self.db.insert_agent_action(
            account_id,
            spec.kind(),
            &summary,
            &payload,
            &preview_json,
            "pending",
            now(),
            None,
        )?;
        self.emit_agent_actions_changed();
        self.db.agent_action(id)?.ok_or_else(|| anyhow!("Failed to load queued agent action"))
    }

    /// Logs an action that ran immediately in Auto mode for the Recent activity list.
    pub fn record_auto_agent_action(
        &self,
        account_id: Option<i64>,
        spec: AgentActionSpec,
        summary: String,
        preview: AgentActionPreview,
    ) -> Result<()> {
        let payload = serde_json::to_string(&spec)?;
        let preview_json = serde_json::to_string(&preview)?;
        let t = now();
        self.db.insert_agent_action(
            account_id,
            spec.kind(),
            &summary,
            &payload,
            &preview_json,
            "auto",
            t,
            Some(t),
        )?;
        let _ = self.db.prune_resolved_agent_actions(MAX_RESOLVED_HISTORY);
        self.emit_agent_actions_changed();
        Ok(())
    }

    /// Pending actions (oldest first) followed by up to `recent_limit` resolved/auto actions.
    pub fn agent_actions(&self, recent_limit: usize) -> Result<Vec<AgentAction>> {
        self.db.agent_actions(recent_limit)
    }

    pub fn pending_agent_actions_count(&self) -> Result<usize> {
        self.db.pending_agent_actions_count()
    }

    /// Approves and executes a pending agent action.
    pub fn approve_agent_action(&self, id: i64) -> Result<AgentAction> {
        let action = self.db.agent_action(id)?.ok_or_else(|| anyhow!("No pending action {id}"))?;
        if action.status != "pending" {
            return Err(anyhow!("Action {id} has already been {}", action.status));
        }
        let outcome = self.execute_agent_action(&action.spec);
        let t = now();
        match outcome {
            Ok(()) => {
                self.db.resolve_agent_action(id, "approved", None, t)?;
                let _ = self.db.prune_resolved_agent_actions(MAX_RESOLVED_HISTORY);
                self.emit_agent_actions_changed();
                self.db.agent_action(id)?.ok_or_else(|| anyhow!("Action {id} disappeared"))
            }
            Err(e) => {
                let msg = format!("{e:#}");
                let _ = self.db.resolve_agent_action(id, "failed", Some(&msg), t);
                let _ = self.db.prune_resolved_agent_actions(MAX_RESOLVED_HISTORY);
                self.emit_agent_actions_changed();
                Err(e)
            }
        }
    }

    /// Rejects a pending agent action without executing it.
    pub fn reject_agent_action(&self, id: i64) -> Result<()> {
        if !self.db.resolve_agent_action(id, "rejected", None, now())? {
            return Err(anyhow!("No pending action {id}"));
        }
        let _ = self.db.prune_resolved_agent_actions(MAX_RESOLVED_HISTORY);
        self.emit_agent_actions_changed();
        Ok(())
    }

    /// Approves every pending agent action in FIFO order. One failure doesn't stop the rest.
    pub fn approve_all_agent_actions(&self) -> Result<BatchOutcome> {
        let ids = self.db.pending_agent_action_ids()?;
        let mut out = BatchOutcome::default();
        for id in ids {
            match self.approve_agent_action(id) {
                Ok(_) => out.succeeded += 1,
                Err(e) => {
                    out.failed += 1;
                    out.errors.push(format!("{e:#}"));
                }
            }
        }
        Ok(out)
    }

    /// Rejects every pending agent action. Returns how many were rejected.
    pub fn reject_all_agent_actions(&self) -> Result<usize> {
        let n = self.db.reject_all_pending_agent_actions(now())?;
        let _ = self.db.prune_resolved_agent_actions(MAX_RESOLVED_HISTORY);
        self.emit_agent_actions_changed();
        Ok(n)
    }

    fn live_ids(&self, ids: &[i64]) -> Vec<i64> {
        ids.iter().copied().filter(|id| self.db.locate(*id).is_ok()).collect()
    }

    fn execute_agent_action(&self, spec: &AgentActionSpec) -> Result<()> {
        match spec {
            AgentActionSpec::MarkSeen { ids, seen } => {
                let live = self.live_ids(ids);
                if live.is_empty() {
                    return Err(anyhow!("Those messages are no longer available"));
                }
                self.set_seen(&live, *seen)
            }
            AgentActionSpec::SetFlagged { ids, flagged } => {
                let live = self.live_ids(ids);
                if live.is_empty() {
                    return Err(anyhow!("Those messages are no longer available"));
                }
                self.set_flagged(&live, *flagged)
            }
            AgentActionSpec::Archive { ids } => {
                let live = self.live_ids(ids);
                if live.is_empty() {
                    return Err(anyhow!("Those messages are no longer available"));
                }
                self.archive(&live)
            }
            AgentActionSpec::Trash { ids } => {
                let live: Vec<i64> = self
                    .live_ids(ids)
                    .into_iter()
                    .filter(|id| self.message_folder_role(*id).ok().and_then(|(_, _, r)| r).as_deref() != Some("trash"))
                    .collect();
                if live.is_empty() {
                    return Err(anyhow!("Those messages are already in Trash or no longer available"));
                }
                self.move_to_trash_only(&live)?;
                Ok(())
            }
            AgentActionSpec::Junk { ids, junk } => {
                let live = self.live_ids(ids);
                if live.is_empty() {
                    return Err(anyhow!("Those messages are no longer available"));
                }
                if *junk { self.junk(&live) } else { self.not_junk(&live) }
            }
            AgentActionSpec::MoveTo { ids, folder_id } => {
                let live = self.live_ids(ids);
                if live.is_empty() {
                    return Err(anyhow!("Those messages are no longer available"));
                }
                self.move_to(&live, *folder_id)
            }
            AgentActionSpec::Snooze { ids, until } => {
                if *until <= now() {
                    return Err(anyhow!("The snooze time has already passed"));
                }
                let live = self.live_ids(ids);
                if live.is_empty() {
                    return Err(anyhow!("Those messages are no longer available"));
                }
                self.snooze(&live, *until)
            }
            AgentActionSpec::Unsnooze { ids } => {
                let live = self.live_ids(ids);
                if live.is_empty() {
                    return Err(anyhow!("Those messages are no longer available"));
                }
                self.unsnooze(&live)
            }
            AgentActionSpec::DeleteDraft { account_id, message_id } => self.delete_draft(*account_id, message_id),
            AgentActionSpec::CreateRule { rule } => {
                let mut r = rule.clone();
                r.id = 0;
                self.save_rule_with_approval(r, true, true)?;
                Ok(())
            }
            AgentActionSpec::UpdateRule { rule } => {
                // Ensure the rule still exists before updating.
                self.rule(rule.id)?;
                self.save_rule_with_approval(rule.clone(), true, true)?;
                Ok(())
            }
            AgentActionSpec::DeleteRule { id } => self.delete_rule(*id),
            AgentActionSpec::SendMail { .. } => Err(anyhow!("SendMail actions cannot be executed from the queue")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_engine() -> (Engine, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("mailvern-agent-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let engine = Engine::open(&dir.join("mailvern.db")).unwrap();
        (engine, dir)
    }

    fn acc(email: &str) -> Account {
        Account {
            id: 0,
            name: "Alex".into(),
            email: email.into(),
            imap_host: "imap.example.com".into(),
            imap_port: 993,
            smtp_host: "smtp.example.com".into(),
            smtp_port: 465,
            color: String::new(),
            signature: String::new(),
            position: 0,
            label: String::new(),
        }
    }

    #[test]
    fn permission_resolution_supports_default_and_per_account_overrides() {
        let (engine, dir) = temp_engine();
        let a1 = engine.seed(&acc("work@acme.co"), &[("INBOX", Some("inbox"), vec![])]).unwrap();
        let a2 = engine.seed(&acc("personal@me.co"), &[("INBOX", Some("inbox"), vec![])]).unwrap();

        // Default is Approval for all accounts.
        let p = engine.agent_permissions().unwrap();
        assert_eq!(p.default_mode, AgentMode::Approval);
        assert_eq!(p.for_account(a1), AgentMode::Approval);
        assert_eq!(p.for_account(a2), AgentMode::Approval);
        assert!(!p.any_auto(&[a1, a2]));
        assert!(!p.all_auto(&[a1, a2]));

        // Override only personal to Auto.
        engine.set_agent_account_mode(a2, Some(AgentMode::Auto)).unwrap();
        let p = engine.agent_permissions().unwrap();
        assert_eq!(p.for_account(a1), AgentMode::Approval);
        assert_eq!(p.for_account(a2), AgentMode::Auto);
        assert!(p.any_auto(&[a1, a2]));
        assert!(!p.all_auto(&[a1, a2]));

        // Flip default to Auto, keep work pinned to Approval.
        engine.set_agent_default_mode(AgentMode::Auto).unwrap();
        engine.set_agent_account_mode(a1, Some(AgentMode::Approval)).unwrap();
        engine.set_agent_account_mode(a2, None).unwrap();
        let p = engine.agent_permissions().unwrap();
        assert_eq!(p.for_account(a1), AgentMode::Approval);
        assert_eq!(p.for_account(a2), AgentMode::Auto);
        assert!(!p.all_auto(&[a1, a2]));

        // Clear work override: now every account is Auto.
        engine.set_agent_account_mode(a1, None).unwrap();
        let p = engine.agent_permissions().unwrap();
        assert!(p.all_auto(&[a1, a2]));

        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn move_to_trash_only_refuses_messages_already_in_trash() {
        let (engine, dir) = temp_engine();
        let raw = b"From: Maya <maya@x.io>\r\nSubject: Hi\r\nMessage-ID: <1@x.io>\r\n\r\nHello";
        let _a = engine
            .seed(&acc("work@acme.co"), &[("INBOX", Some("inbox"), vec![raw]), ("Trash", Some("trash"), vec![raw])])
            .unwrap();
        let trash_folder = engine.folders().unwrap().into_iter().find(|f| f.role.as_deref() == Some("trash")).unwrap();
        let msgs = engine.messages(&View::Folder { folder_id: trash_folder.id }, None, 0, 10, false).unwrap();
        assert_eq!(msgs.len(), 1);
        let err = engine.move_to_trash_only(&[msgs[0].id]).unwrap_err().to_string();
        assert!(err.contains("already in Trash"), "{err}");
        // Message is still untouched in Trash.
        assert_eq!(engine.messages(&View::Folder { folder_id: trash_folder.id }, None, 0, 10, false).unwrap().len(), 1);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn queue_approve_reject_and_expired_snooze_work() {
        let (engine, dir) = temp_engine();
        let a1 = engine.seed(&acc("work@acme.co"), &[("INBOX", Some("inbox"), vec![])]).unwrap();

        let rule = Rule {
            id: 0,
            name: "Forward receipts".into(),
            enabled: true,
            account_id: Some(a1),
            match_all: true,
            conditions: vec![RuleCondition { field: RuleField::Subject, op: RuleOp::Contains, value: "receipt".into() }],
            exceptions: vec![],
            actions: vec![RuleAction::Forward { to: vec!["acc@acme.co".into()] }],
            stop: false,
            origin: "agent".into(),
            needs_approval: false,
            paused_reason: None,
        };
        let queued = engine
            .queue_agent_action(Some(a1), AgentActionSpec::CreateRule { rule }, "Add rule “Forward receipts”".into(), AgentActionPreview::default())
            .unwrap();
        assert_eq!(engine.pending_agent_actions_count().unwrap(), 1);
        assert!(engine.rules().unwrap().is_empty());

        // Approving saves the rule live without waiting for a second approval.
        let done = engine.approve_agent_action(queued.id).unwrap();
        assert_eq!(done.status, "approved");
        assert_eq!(engine.pending_agent_actions_count().unwrap(), 0);
        let saved = engine.rules().unwrap();
        assert_eq!(saved.len(), 1);
        assert!(saved[0].enabled && !saved[0].needs_approval && saved[0].origin == "agent");

        // Rejecting leaves the rule intact.
        let del = engine
            .queue_agent_action(Some(a1), AgentActionSpec::DeleteRule { id: saved[0].id }, "Remove rule".into(), AgentActionPreview::default())
            .unwrap();
        engine.reject_agent_action(del.id).unwrap();
        assert_eq!(engine.rules().unwrap().len(), 1);

        // Expired snooze fails cleanly and marks the action failed.
        let snooze = engine
            .queue_agent_action(Some(a1), AgentActionSpec::Snooze { ids: vec![1], until: 100 }, "Snooze".into(), AgentActionPreview::default())
            .unwrap();
        let err = engine.approve_agent_action(snooze.id).unwrap_err().to_string();
        assert!(err.contains("already passed"), "{err}");
        let all = engine.agent_actions(10).unwrap();
        assert_eq!(all.iter().find(|x| x.id == snooze.id).unwrap().status, "failed");

        std::fs::remove_dir_all(dir).ok();
    }
}
