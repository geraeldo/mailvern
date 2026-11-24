//! Rules on the engine: running them on new Inbox mail, and the API the UI and agents use
//! to manage them. Actions go through the same methods as the user's own clicks.

use super::{now, Engine};
use crate::model::*;
use crate::rules::{self, Compiled, Facts, Verdict};
use anyhow::{anyhow, Result};
use std::collections::HashMap;

/// Forwards and replies one rule may send per hour before it's paused.
const MAX_SENDS_PER_HOUR: i64 = 50;
/// A rule auto-replies to the same address at most once in this many seconds.
const REPLY_EVERY: i64 = 4 * 86_400;
/// How long rules remember a message they've seen.
const SEEN_FOR: i64 = 90 * 86_400;
/// Queued mail older than this (e.g. after a crash) is still processed but not announced.
const STALE_AFTER: i64 = 600;
/// How many recent messages a test or a manual run looks at.
const SCAN_LIMIT: usize = 2000;

/// What the rules did to one message.
#[derive(Default)]
struct Applied {
    /// It left the Inbox.
    relocated: bool,
    read: bool,
    quiet: bool,
}

impl Engine {
    // ---------- managing rules ----------

    /// Every rule, in the order they run.
    pub fn rules(&self) -> Result<Vec<Rule>> {
        self.db.rules()
    }

    /// The headers rules look at, for these messages (missing ones are left out). The AI
    /// that writes rules reads these as examples.
    pub fn rule_facts(&self, ids: &[i64]) -> Result<Vec<Facts>> {
        ids.iter().filter_map(|id| self.db.rule_facts(*id).transpose()).collect()
    }

    pub fn rule(&self, id: i64) -> Result<Rule> {
        self.db.rule(id)?.ok_or_else(|| anyhow!("There is no rule {id}"))
    }

    fn my_addresses(&self) -> Result<Vec<String>> {
        Ok(self.db.accounts()?.into_iter().map(|a| a.email).collect())
    }

    fn rules_changed(&self) {
        let pending_approval = self.db.rules_waiting_for_approval().unwrap_or(0);
        let _ = self.events.send(Event::RulesChanged { pending_approval });
    }

    /// Checks a rule without saving it. The error says what to fix.
    pub fn check_rule(&self, rule: &Rule) -> Result<()> {
        if let Some(a) = rule.account_id {
            self.db.account(a).map_err(|_| anyhow!("There is no account {a}"))?;
        }
        rules::validate(rule, &self.db.folders()?, &self.my_addresses()?).map_err(|e| anyhow!(e))
    }

    /// Adds a rule (id 0) or replaces one. Returns it as stored.
    ///
    /// When an agent saves a rule that sends mail (forward/reply) or relocates/deletes mail
    /// (move/archive/trash/junk/snooze), any new rule, re-enable, or change to its scope,
    /// conditions, exceptions, or actions is stored switched off and waiting for the user's
    /// approval in Mailvern.
    pub fn save_rule(&self, mut rule: Rule, by_agent: bool) -> Result<Rule> {
        rule.name = rule.name.trim().to_string();
        self.check_rule(&rule)?;
        let old = if rule.id > 0 { Some(self.rule(rule.id)?) } else { None };
        rule.paused_reason = None;
        if by_agent {
            let needs = rule.actions.iter().any(RuleAction::requires_approval);
            let unapproved = match &old {
                None => true,
                Some(o) => {
                    o.needs_approval
                        || (!o.enabled && rule.enabled)
                        || o.account_id != rule.account_id
                        || o.match_all != rule.match_all
                        || o.conditions != rule.conditions
                        || o.exceptions != rule.exceptions
                        || o.actions != rule.actions
                }
            };
            rule.origin = "agent".into();
            rule.needs_approval = needs && unapproved;
            if rule.needs_approval {
                rule.enabled = false;
            }
        } else {
            rule.origin = "user".into();
            rule.needs_approval = false;
        }
        let t = now();
        if old.is_some() {
            self.db.update_rule(&rule, t)?;
        } else {
            rule.id = self.db.insert_rule(&rule, t)?;
        }
        self.rules_changed();
        self.rule(rule.id)
    }

    pub fn delete_rule(&self, id: i64) -> Result<()> {
        if !self.db.delete_rule(id)? {
            return Err(anyhow!("There is no rule {id}"));
        }
        self.rules_changed();
        Ok(())
    }

    pub fn reorder_rules(&self, ids: &[i64]) -> Result<()> {
        self.db.reorder_rules(ids)?;
        self.rules_changed();
        Ok(())
    }

    /// Turns a rule on or off. Turning it on also approves it and clears a pause, so only
    /// the user may do it for a rule that waits for approval.
    pub fn set_rule_enabled(&self, id: i64, on: bool) -> Result<()> {
        let rule = self.rule(id)?;
        let pending = if on { false } else { rule.needs_approval };
        self.db.set_rule_state(id, on, pending, if on { None } else { rule.paused_reason.as_deref() })?;
        self.rules_changed();
        Ok(())
    }

    fn pause_rule(&self, rule: &Rule, reason: &str) {
        if self.db.set_rule_state(rule.id, false, rule.needs_approval, Some(reason)).is_ok() {
            let _ = self.events.send(Event::RulePaused { rule_id: rule.id, name: rule.name.clone(), reason: reason.into() });
            self.rules_changed();
        }
    }

    // ---------- trying rules on mail that's already here ----------

    /// Which recent messages a rule matches: in `folder_id`, or else the Inbox of the
    /// rule's account (every Inbox when it has none). Doesn't download anything.
    pub fn test_rule(&self, rule: &Rule, folder_id: Option<i64>) -> Result<RuleTest> {
        let compiled = Compiled::new(rule, &self.my_addresses()?).map_err(|e| anyhow!(e))?;
        let candidates = self.db.rule_candidates(rule.account_id, folder_id, SCAN_LIMIT)?;
        let scanned = candidates.len();
        let mut without_body = 0;
        let mut hits = Vec::new();
        for f in &candidates {
            match self.check_stored(&compiled, f)? {
                Some(true) => hits.push(f.id),
                Some(false) => {}
                None => without_body += 1,
            }
        }
        let sample = self.db.summaries(&hits[..hits.len().min(20)])?;
        Ok(RuleTest { count: hits.len(), scanned, without_body, sample })
    }

    /// None: it depends on a body that hasn't been downloaded.
    fn check_stored(&self, c: &Compiled, f: &Facts) -> Result<Option<bool>> {
        Ok(match c.check(f, None) {
            Verdict::Yes => Some(true),
            Verdict::No => Some(false),
            Verdict::NeedBody => match self.db.body_text(f.id)? {
                Some(b) => Some(c.check(f, Some(&b)) == Verdict::Yes),
                None => None,
            },
        })
    }

    /// Runs a rule on recent mail already here (see `test_rule` for which). Forwards and
    /// replies go out only with `include_sending`, within the usual limits.
    pub async fn run_rule(&self, id: i64, folder_id: Option<i64>, include_sending: bool) -> Result<RuleRun> {
        let rule = self.rule(id)?;
        if rule.needs_approval {
            return Err(anyhow!("Approve and turn on this rule before running it"));
        }
        if let (Some(acc), Some(fid)) = (rule.account_id, folder_id) {
            let (folder_acc, ..) = self.db.folder(fid)?;
            if folder_acc != acc {
                return Err(anyhow!("That folder belongs to another account than the rule"));
            }
        }
        let mine = self.my_addresses()?;
        let compiled = Compiled::new(&rule, &mine).map_err(|e| anyhow!(e))?;
        let mut run = RuleRun::default();
        for f in self.db.rule_candidates(rule.account_id, folder_id, SCAN_LIMIT)? {
            if self.check_stored(&compiled, &f)? == Some(true) {
                run.matched += 1;
                self.apply(&[&rule], &f, include_sending, &mine, &mut run).await;
            }
        }
        Ok(run)
    }

    // ---------- the activity log ----------

    pub fn rule_activity(&self, limit: usize) -> Result<Vec<RuleActivity>> {
        Ok(self.db.rule_activity(limit)?.into_iter().map(|(a, _)| a).collect())
    }

    /// Reverses one thing a rule did: brings the message back to the Inbox, or marks it
    /// unread or unflagged again. Sent mail can't be taken back.
    pub fn undo_rule_activity(&self, id: i64) -> Result<()> {
        let (a, msgid) = self.db.rule_log_entry(id)?.ok_or_else(|| anyhow!("That activity is gone"))?;
        let Some(msgid) = msgid.filter(|_| a.can_undo) else {
            return Err(anyhow!("This can't be undone"));
        };
        let acc = a.account_id;
        let back_to_inbox = |folder_id: Option<i64>, place: &str| -> Result<()> {
            let ids = match folder_id {
                Some(f) => self.db.ids_by_message_id(f, &msgid)?,
                None => vec![],
            };
            if ids.is_empty() {
                return Err(anyhow!("Couldn't find the message in {place} yet. Try again in a moment."));
            }
            let inbox = self.db.folder_by_role(acc, "inbox")?.ok_or_else(|| anyhow!("This account has no Inbox"))?;
            self.move_to(&ids, inbox.0)
        };
        let role_folder = |role: &str| -> Result<Option<i64>> { Ok(self.db.folder_by_role(acc, role)?.map(|f| f.0)) };
        match &a.action {
            RuleAction::Move { folder_id } => back_to_inbox(Some(*folder_id), "that folder")?,
            RuleAction::Archive => back_to_inbox(role_folder("archive")?, "Archive")?,
            RuleAction::Trash => back_to_inbox(role_folder("trash")?, "Trash")?,
            RuleAction::Junk => back_to_inbox(role_folder("junk")?, "Junk")?,
            RuleAction::Snooze { .. } => self.unsnooze(&self.db.ids_in_account(acc, &msgid)?)?,
            RuleAction::MarkRead => self.set_seen(&self.db.ids_in_account(acc, &msgid)?, false)?,
            RuleAction::Flag => self.set_flagged(&self.db.ids_in_account(acc, &msgid)?, false)?,
            _ => return Err(anyhow!("This can't be undone")),
        }
        self.db.mark_rule_undone(id)
    }

    // ---------- running on new mail ----------

    /// Takes new Inbox mail off the queue as it arrives, runs the rules on it, then
    /// announces what's still in the Inbox and unread.
    pub(super) async fn rule_runner(self) {
        let _ = self.db.prune_rule_seen(now() - SEEN_FOR);
        loop {
            let batch = self.db.rule_queue(100).unwrap_or_else(|e| {
                eprintln!("rules: {e:#}");
                vec![]
            });
            if batch.is_empty() {
                self.rules_wake.notified().await;
                continue;
            }
            self.process_queue(batch).await;
        }
    }

    /// (message id, account id, queued at) rows from the queue.
    async fn process_queue(&self, batch: Vec<(i64, i64, i64)>) {
        let mine = self.my_addresses().unwrap_or_default();
        let mut announce: HashMap<i64, Vec<Facts>> = HashMap::new();
        for (id, account_id, queued_at) in batch {
            let facts = self.db.rule_facts(id);
            let _ = self.db.unqueue_for_rules(&[id]);
            let f = match facts {
                Ok(Some(f)) => f,
                Ok(None) => continue,
                Err(e) => {
                    eprintln!("rules: {e:#}");
                    continue;
                }
            };
            let applied = self.run_rules_on(&f, &mine).await.unwrap_or_else(|e| {
                eprintln!("rules: {e:#}");
                Applied::default()
            });
            if !f.seen && !applied.relocated && !applied.read && !applied.quiet && now() - queued_at < STALE_AFTER {
                announce.entry(account_id).or_default().push(f);
            }
        }
        for (account_id, list) in announce {
            if let Some(last) = list.iter().max_by_key(|f| f.date) {
                let _ = self.events.send(Event::NewMail {
                    account_id,
                    from: if last.from_name.is_empty() { last.from_addr.clone() } else { last.from_name.clone() },
                    subject: last.subject.clone(),
                    count: list.len(),
                });
            }
        }
    }

    async fn run_rules_on(&self, f: &Facts, mine: &[String]) -> Result<Applied> {
        // A message that comes back to the Inbox gets a new UID; rules saw it already.
        if let Some(mid) = &f.message_id {
            if !self.db.mark_rule_seen(f.account_id, mid, now())? {
                return Ok(Applied::default());
            }
        }
        let rules: Vec<Rule> = self
            .db
            .rules()?
            .into_iter()
            .filter(|r| r.enabled && !r.needs_approval && r.paused_reason.is_none())
            .filter(|r| r.account_id.is_none_or(|a| a == f.account_id))
            .collect();
        let mut body: Option<String> = None;
        let mut matched = Vec::new();
        for r in &rules {
            let Ok(c) = Compiled::new(r, mine) else { continue };
            let mut v = c.check(f, body.as_deref());
            if v == Verdict::NeedBody && body.is_none() {
                // Download it only now that a rule needs it. If that fails, it counts as empty.
                if self.db.body_text(f.id)?.is_none() {
                    if let Err(e) = self.raw(f.id).await {
                        eprintln!("rules: couldn't load a body: {e:#}");
                    }
                }
                body = Some(self.db.body_text(f.id)?.unwrap_or_default());
                v = c.check(f, body.as_deref());
            }
            if v == Verdict::Yes {
                matched.push(r);
                if r.stop {
                    break;
                }
            }
        }
        if matched.is_empty() {
            return Ok(Applied::default());
        }
        Ok(self.apply(&matched, f, true, mine, &mut RuleRun::default()).await)
    }

    fn log(&self, rule: &Rule, f: &Facts, action: &RuleAction) {
        if let Err(e) = self.db.log_rule(rule, f.account_id, f.message_id.as_deref(), &f.subject, &f.from_addr, action, now()) {
            eprintln!("rules: {e:#}");
        }
    }

    /// Carries out what the matching rules ask, in this order: mail is sent while the
    /// message is still where it was, then flags change, then it moves (to the first
    /// place any rule names). A failing step is logged and the rest still happens.
    async fn apply(&self, matched: &[&Rule], f: &Facts, include_sending: bool, mine: &[String], run: &mut RuleRun) -> Applied {
        let mut applied = Applied::default();
        for rule in matched {
            for action in rule.actions.iter().filter(|a| a.sends()) {
                if !include_sending {
                    run.not_sent += 1;
                    continue;
                }
                match self.send_for_rule(rule, action, f, mine).await {
                    Ok(true) => run.sent += 1,
                    Ok(false) => run.not_sent += 1,
                    Err(e) => {
                        eprintln!("rules: “{}” couldn't send: {e:#}", rule.name);
                        run.not_sent += 1;
                    }
                }
            }
        }
        let first = |pick: fn(&RuleAction) -> bool| {
            matched.iter().find_map(|r| r.actions.iter().find(|a| pick(a)).map(|a| (*r, a)))
        };
        if let Some((rule, a)) = first(|a| *a == RuleAction::MarkRead) {
            applied.read = true;
            if !f.seen {
                match self.set_seen(&[f.id], true) {
                    Ok(()) => self.log(rule, f, a),
                    Err(e) => eprintln!("rules: {e:#}"),
                }
            }
        }
        if let Some((rule, a)) = first(|a| *a == RuleAction::Flag) {
            match self.set_flagged(&[f.id], true) {
                Ok(()) => self.log(rule, f, a),
                Err(e) => eprintln!("rules: {e:#}"),
            }
        }
        if let Some((rule, a)) = first(|a| *a == RuleAction::SkipNotification) {
            applied.quiet = true;
            self.log(rule, f, a);
        }
        if let Some((rule, a)) = first(RuleAction::relocates) {
            let ids = [f.id];
            let res = match a {
                RuleAction::Move { folder_id } => self.move_to(&ids, *folder_id),
                RuleAction::Archive => self.archive(&ids),
                RuleAction::Trash => self.trash(&ids),
                RuleAction::Junk => self.junk(&ids),
                RuleAction::Snooze { until } => self.snooze(&ids, rules::snooze_until(*until, chrono::Local::now())),
                _ => Ok(()),
            };
            match res {
                Ok(()) => {
                    applied.relocated = true;
                    self.log(rule, f, a);
                }
                Err(e) => eprintln!("rules: “{}”: {e:#}", rule.name),
            }
        }
        applied
    }

    /// Sends one forward or auto-reply. Ok(false): left out on purpose (a machine sent the
    /// message, a list, a limit).
    async fn send_for_rule(&self, rule: &Rule, action: &RuleAction, f: &Facts, mine: &[String]) -> Result<bool> {
        let refused = match action {
            RuleAction::Forward { .. } => rules::may_forward(f),
            RuleAction::Reply { .. } => rules::may_reply(f, mine),
            _ => return Ok(false),
        };
        if refused.is_err() {
            return Ok(false);
        }
        if self.db.rule_sends_since(rule.id, now() - 3600)? >= MAX_SENDS_PER_HOUR {
            self.pause_rule(rule, &format!("it sent {MAX_SENDS_PER_HOUR} messages in an hour"));
            return Ok(false);
        }
        match action {
            RuleAction::Forward { to } => {
                let detail = self.message(f.id).await?;
                let files = self.all_attachments(f.id).await?;
                let dir = std::env::temp_dir().join(format!("mailvern-forward-{}", uuid::Uuid::new_v4()));
                let mut paths = Vec::new();
                if !files.is_empty() {
                    std::fs::create_dir_all(&dir)?;
                    for (i, (name, bytes)) in files.iter().enumerate() {
                        let safe = crate::text::safe_filename(name);
                        let sub = dir.join(i.to_string());
                        std::fs::create_dir_all(&sub)?;
                        let path = sub.join(safe);
                        std::fs::write(&path, bytes)?;
                        paths.push(path.to_string_lossy().to_string());
                    }
                }
                // The message is built (attachments read) when it's queued, so the files can go.
                let res = self.send(rules::forward_outgoing(&detail, f.account_id, to, paths), Some(now()));
                let _ = std::fs::remove_dir_all(&dir);
                res?;
            }
            RuleAction::Reply { subject, text } => {
                let addr = rules::reply_address(f);
                if self.db.replied_since(rule.id, &addr, now() - REPLY_EVERY)? {
                    return Ok(false);
                }
                self.send(rules::reply_outgoing(f, f.account_id, subject.as_deref(), text), Some(now()))?;
                self.db.record_reply(rule.id, &addr, now())?;
            }
            _ => unreachable!(),
        }
        self.log(rule, f, action);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::super::Handle;
    use super::*;
    use crate::worker::{Cmd, Target};
    use tokio::sync::mpsc;

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
            signature: String::new(),
            position: 0,
            label: String::new(),
        }
    }

    fn mail(from: &str, subject: &str, mid: &str, extra: &str) -> Vec<u8> {
        format!(
            "From: {from}\r\nTo: me@example.com\r\nSubject: {subject}\r\nMessage-ID: <{mid}>\r\nDate: Mon, 23 Nov 2026 10:00:00 +0000\r\n{extra}\r\nHello there\r\n"
        )
        .into_bytes()
    }

    struct Setup {
        engine: Engine,
        account: i64,
        rx: mpsc::UnboundedReceiver<Cmd>,
    }

    /// An engine with one account (Inbox, Archive, Dev) and a stand-in worker that
    /// collects the commands sent to the server.
    fn setup(inbox: Vec<Vec<u8>>) -> Setup {
        let dir = std::env::temp_dir().join(format!("mailvern-rules-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let engine = Engine::open(&dir.join("mailvern.db")).unwrap();
        let refs: Vec<&[u8]> = inbox.iter().map(|m| m.as_slice()).collect();
        let account = engine.seed(&account(), &[("INBOX", Some("inbox"), refs), ("Archive", Some("archive"), vec![]), ("Dev", None, vec![])]).unwrap();
        let (tx, rx) = mpsc::unbounded_channel();
        engine.workers.lock().unwrap().insert(account, Handle { tx, tasks: vec![] });
        Setup { engine, account, rx }
    }

    fn inbox_ids(e: &Engine, account: i64) -> Vec<i64> {
        let inbox = e.db.folder_by_role(account, "inbox").unwrap().unwrap().0;
        e.db.rule_candidates(None, Some(inbox), 100).unwrap().iter().map(|f| f.id).collect()
    }

    fn rule(name: &str, conditions: Vec<RuleCondition>, actions: Vec<RuleAction>) -> Rule {
        Rule {
            id: 0,
            name: name.into(),
            enabled: true,
            account_id: None,
            match_all: true,
            conditions,
            exceptions: vec![],
            actions,
            stop: false,
            origin: String::new(),
            needs_approval: false,
            paused_reason: None,
        }
    }

    fn from(v: &str) -> RuleCondition {
        RuleCondition { field: RuleField::From, op: RuleOp::Contains, value: v.into() }
    }

    async fn run_queue(s: &Setup, ids: &[i64]) {
        s.engine.db.queue_for_rules(s.account, ids, now()).unwrap();
        let batch = s.engine.db.rule_queue(100).unwrap();
        s.engine.process_queue(batch).await;
    }

    #[tokio::test]
    async fn new_mail_is_archived_once_and_not_announced() {
        // seed marks the first message read, the second unread.
        let mut s = setup(vec![
            mail("Pat <pat@example.org>", "Lunch?", "a@x", ""),
            mail("GitHub <notifications@github.com>", "PR #12", "b@x", "List-Id: <repo.github.com>\r\n"),
        ]);
        let saved = s.engine.save_rule(rule("GitHub", vec![from("github")], vec![RuleAction::Archive]), false).unwrap();
        assert!(saved.id > 0 && saved.enabled);
        let mut events = s.engine.subscribe();
        let ids = inbox_ids(&s.engine, s.account);
        assert_eq!(ids.len(), 2);
        run_queue(&s, &ids).await;

        // The GitHub mail left the Inbox and the server was told to move it.
        assert_eq!(inbox_ids(&s.engine, s.account).len(), 1);
        assert!(matches!(s.rx.try_recv().unwrap(), Cmd::Move { to: Target::Role("archive"), .. }));
        // Only unread mail still in the Inbox is announced: Pat's is read, GitHub's is gone.
        while let Ok(ev) = events.try_recv() {
            assert!(!matches!(ev, Event::NewMail { .. }), "nothing should be announced");
        }
        let log = s.engine.rule_activity(10).unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].action, RuleAction::Archive);
        assert!(log[0].can_undo);

        // The same message coming back (new UID, same Message-ID) isn't processed again.
        let raw = mail("GitHub <notifications@github.com>", "PR #12", "b@x", "");
        let inbox = s.engine.db.folder_by_role(s.account, "inbox").unwrap().unwrap().0;
        let back = s.engine.db.insert_messages(s.account, inbox, &[crate::parse::parse_header(9, &raw, false, false, 10, now())]).unwrap();
        run_queue(&s, &back).await;
        assert_eq!(inbox_ids(&s.engine, s.account).len(), 2);
        assert!(s.rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn unread_mail_no_rule_touches_is_announced() {
        let s = setup(vec![mail("A <a@example.org>", "One", "1@x", ""), mail("Pat <pat@example.org>", "Two", "2@x", "")]);
        s.engine.save_rule(rule("Other", vec![from("nobody")], vec![RuleAction::Archive]), false).unwrap();
        let mut events = s.engine.subscribe();
        run_queue(&s, &inbox_ids(&s.engine, s.account)).await;
        let mut announced = None;
        while let Ok(ev) = events.try_recv() {
            if let Event::NewMail { count, subject, .. } = ev {
                announced = Some((count, subject));
            }
        }
        assert_eq!(announced, Some((1, "Two".to_string())));
    }

    #[tokio::test]
    async fn sending_rules_from_agents_wait_for_approval() {
        let s = setup(vec![]);
        let fwd = vec![RuleAction::Forward { to: vec!["pat@example.org".into()] }];
        let r = s.engine.save_rule(rule("Forward", vec![from("boss")], fwd.clone()), true).unwrap();
        assert!(r.needs_approval && !r.enabled && r.origin == "agent");
        // The agent re-saving it, even switched on, doesn't approve it.
        let r = s.engine.save_rule(Rule { enabled: true, ..r }, true).unwrap();
        assert!(r.needs_approval && !r.enabled);
        // The user turning it on does.
        s.engine.set_rule_enabled(r.id, true).unwrap();
        let r = s.engine.rule(r.id).unwrap();
        assert!(!r.needs_approval && r.enabled);
        // An agent editing only the name keeps it approved; widening conditions or changing the address doesn't.
        let r = s.engine.save_rule(Rule { name: "Forward boss".into(), ..r }, true).unwrap();
        assert!(!r.needs_approval && r.enabled);
        let widened = s.engine.save_rule(Rule { conditions: vec![], ..r.clone() }, true).unwrap();
        assert!(widened.needs_approval && !widened.enabled);
        s.engine.set_rule_enabled(r.id, true).unwrap();
        let r = s.engine.rule(r.id).unwrap();
        let r = s.engine.save_rule(Rule { actions: vec![RuleAction::Forward { to: vec!["eve@example.net".into()] }], ..r }, true).unwrap();
        assert!(r.needs_approval && !r.enabled);
        // Unapproved rules cannot be executed via run_rule.
        assert!(s.engine.run_rule(r.id, None, true).await.is_err());
        // An agent can't switch back on a sending rule the user switched off.
        s.engine.set_rule_enabled(r.id, true).unwrap();
        s.engine.set_rule_enabled(r.id, false).unwrap();
        let r = s.engine.save_rule(Rule { enabled: true, ..s.engine.rule(r.id).unwrap() }, true).unwrap();
        assert!(r.needs_approval && !r.enabled);
        // Relocating/deleting rules (archive, trash, move, junk, snooze) from agents also wait for approval.
        let trash = s.engine.save_rule(rule("Trash", vec![from("boss")], vec![RuleAction::Trash]), true).unwrap();
        assert!(trash.needs_approval && !trash.enabled);
        let archive = s.engine.save_rule(rule("Archive", vec![from("boss")], vec![RuleAction::Archive]), true).unwrap();
        assert!(archive.needs_approval && !archive.enabled);
        // Non-sending, non-relocating rules (e.g. Flag) are live at once.
        let r = s.engine.save_rule(rule("Flag", vec![from("boss")], vec![RuleAction::Flag]), true).unwrap();
        assert!(!r.needs_approval && r.enabled);
    }

    #[tokio::test]
    async fn rules_waiting_or_paused_do_nothing() {
        let mut s = setup(vec![mail("Boss <boss@example.org>", "Hi", "1@x", ""), mail("Boss <boss@example.org>", "Hi", "2@x", "")]);
        let r = s.engine.save_rule(rule("Archive", vec![from("boss")], vec![RuleAction::Archive]), false).unwrap();
        s.engine.set_rule_enabled(r.id, false).unwrap();
        run_queue(&s, &inbox_ids(&s.engine, s.account)).await;
        assert_eq!(inbox_ids(&s.engine, s.account).len(), 2);
        assert!(s.rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn auto_replies_respect_the_hourly_limit() {
        let s = setup(vec![]);
        let r = s.engine.save_rule(rule("Away", vec![], vec![RuleAction::Reply { subject: None, text: "Away".into() }]), false).unwrap();
        for _ in 0..MAX_SENDS_PER_HOUR {
            s.engine.db.log_rule(&r, s.account, None, "", "", &r.actions[0], now()).unwrap();
        }
        let f = Facts { account_id: s.account, from_addr: "pat@example.org".into(), ..Default::default() };
        let mut events = s.engine.subscribe();
        assert!(!s.engine.send_for_rule(&r, &r.actions[0], &f, &[]).await.unwrap());
        let r = s.engine.rule(r.id).unwrap();
        assert!(!r.enabled && r.paused_reason.is_some());
        assert!(std::iter::from_fn(|| events.try_recv().ok()).any(|e| matches!(e, Event::RulePaused { .. })));
    }

    #[tokio::test]
    async fn test_rule_counts_matches_and_missing_bodies() {
        let s = setup(vec![
            mail("Shop <shop@example.org>", "Your invoice", "1@x", ""),
            mail("Shop <shop@example.org>", "News", "2@x", ""),
            mail("Pat <pat@example.org>", "Invoice?", "3@x", ""),
        ]);
        let t = s.engine.test_rule(&rule("Shop", vec![from("shop")], vec![RuleAction::Archive]), None).unwrap();
        assert_eq!((t.count, t.scanned, t.without_body), (2, 3, 0));
        // Seeded mail has bodies, so body conditions are checked.
        let body = RuleCondition { field: RuleField::Body, op: RuleOp::Contains, value: "hello".into() };
        let t = s.engine.test_rule(&rule("Body", vec![body], vec![RuleAction::Archive]), None).unwrap();
        assert_eq!(t.count, 3);
    }

    #[tokio::test]
    async fn undo_brings_the_message_back() {
        let mut s = setup(vec![mail("Shop <shop@example.org>", "Deal", "1@x", "")]);
        s.engine.save_rule(rule("Shop", vec![from("shop")], vec![RuleAction::Archive]), false).unwrap();
        run_queue(&s, &inbox_ids(&s.engine, s.account)).await;
        let entry = s.engine.rule_activity(1).unwrap().remove(0);
        // Not in Archive locally until the server move syncs it there.
        assert!(s.engine.undo_rule_activity(entry.id).is_err());
        let raw = mail("Shop <shop@example.org>", "Deal", "1@x", "");
        let archive = s.engine.db.folder_by_role(s.account, "archive").unwrap().unwrap().0;
        s.engine.db.insert_messages(s.account, archive, &[crate::parse::parse_header(1, &raw, false, false, 10, now())]).unwrap();
        while s.rx.try_recv().is_ok() {}
        s.engine.undo_rule_activity(entry.id).unwrap();
        assert!(matches!(s.rx.try_recv().unwrap(), Cmd::Move { to: Target::Folder(_), .. }));
        assert!(!s.engine.rule_activity(1).unwrap()[0].can_undo);
    }
}
