use crate::db::{Db, DueSnooze, OutboxRow};
use crate::model::*;
use crate::worker::{idle_loop, Cmd, Target, Worker};
use crate::icons::{self, Icon, Outcome};
use crate::{imap, parse, secrets, smtp};
use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, oneshot, Notify};
use tokio::task::JoinHandle;

/// Setting keys for the Dock badge.
const BADGE_MODE: &str = "badge_mode";
const BADGE_SEEN: &str = "badge_seen_id";
/// Seconds a sent message waits in the outbox so it can be undone ("0" sends at once).
const UNDO_SEND: &str = "undo_send";
const UNDO_SEND_DEFAULT: i64 = 10;
/// "off" stops Mailvern from looking up sender pictures.
const SENDER_ICONS: &str = "sender_icons";

type Workers = Arc<Mutex<HashMap<i64, Handle>>>;
type GroupedIds = HashMap<(i64, i64), Vec<(i64, u32)>>;
pub type SeedFolder<'a> = (&'a str, Option<&'a str>, Vec<&'a [u8]>);
pub type CancelledSend = (Outgoing, Vec<(String, Vec<u8>)>);

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn worker_tx(workers: &Workers, account_id: i64) -> Option<mpsc::UnboundedSender<Cmd>> {
    workers.lock().unwrap_or_else(|e| e.into_inner()).get(&account_id).map(|h| h.tx.clone())
}

struct Handle {
    tx: mpsc::UnboundedSender<Cmd>,
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for Handle {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

#[path = "rule_runner.rs"]
mod rule_runner;

/// The public API the UI talks to. Every read is served from the local database.
/// Cloning is cheap: clones share everything.
#[derive(Clone)]
pub struct Engine {
    db: Arc<Db>,
    events: broadcast::Sender<Event>,
    workers: Workers,
    /// Wakes the scheduler when the outbox or snoozes change.
    wake: Arc<Notify>,
    /// Fetches sender pictures; made on first use (None if the system DNS setup can't be read).
    /// Shared by clones, so there is one fetcher.
    icons: Arc<std::sync::OnceLock<Option<Arc<icons::Fetcher>>>>,
    /// Wakes the rule runner when new Inbox mail is queued for it.
    rules_wake: Arc<Notify>,
}

impl Engine {
    pub fn open(db_path: &Path) -> Result<Self> {
        let (events, _) = broadcast::channel(256);
        Ok(Engine {
            db: Arc::new(Db::open(db_path)?),
            events,
            workers: Arc::new(Mutex::new(HashMap::new())),
            wake: Arc::new(Notify::new()),
            icons: Arc::new(std::sync::OnceLock::new()),
            rules_wake: Arc::new(Notify::new()),
        })
    }

    /// Adds an account with folders (path, role, raw messages) as a sync would have stored
    /// them, without a mail server. For tests and demos of code built on the engine.
    #[doc(hidden)]
    pub fn seed(&self, account: &Account, folders: &[SeedFolder<'_>]) -> Result<i64> {
        let id = self.db.save_account(account)?;
        for (path, role, messages) in folders {
            let folder_id = self.db.upsert_folder(id, path, path, *role)?;
            for (i, raw) in messages.iter().enumerate() {
                let uid = i as u32 + 1;
                let header = parse::parse_header(uid, raw, i % 2 == 0, false, raw.len() as u32, now() - 3600 * (messages.len() - i) as i64);
                self.db.insert_messages(id, folder_id, &[header])?;
                self.db.store_body(folder_id, uid, raw, &parse::body_info(raw))?;
            }
        }
        Ok(id)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    /// Starts a worker for every account. Must be called inside a Tokio runtime.
    pub fn start(&self) -> Result<()> {
        for acc in self.db.accounts()? {
            self.start_account(acc);
        }
        tokio::spawn(scheduler(self.db.clone(), self.events.clone(), self.workers.clone(), self.wake.clone()));
        tokio::spawn(self.clone().rule_runner());
        Ok(())
    }

    fn start_account(&self, acc: Account) {
        let (tx, rx) = mpsc::unbounded_channel();
        let worker = Worker::new(acc.clone(), self.db.clone(), self.events.clone(), self.rules_wake.clone(), rx);
        let _ = tx.send(Cmd::SyncAll);
        let tasks = vec![
            tokio::spawn(worker.run()),
            tokio::spawn(idle_loop(acc.clone(), self.db.clone(), tx.clone())),
        ];
        self.workers.lock().unwrap_or_else(|e| e.into_inner()).insert(acc.id, Handle { tx, tasks });
    }

    fn send_cmd(&self, account_id: i64, cmd: Cmd) -> Result<()> {
        let w = self.workers.lock().unwrap_or_else(|e| e.into_inner());
        let h = w.get(&account_id).ok_or_else(|| anyhow!("Account is not running"))?;
        h.tx.send(cmd).map_err(|_| anyhow!("Account worker stopped"))
    }

    pub fn sync_now(&self) {
        for h in self.workers.lock().unwrap_or_else(|e| e.into_inner()).values() {
            let _ = h.tx.send(Cmd::SyncAll);
        }
    }

    /// Checks every folder of one account.
    pub fn sync_account(&self, account_id: i64) -> Result<()> {
        self.send_cmd(account_id, Cmd::SyncAll)
    }

    /// Checks just the folders behind a view (one folder, or one role across accounts).
    pub fn sync_view(&self, view: &View) -> Result<()> {
        for (acc, folder_id) in self.view_folders(view)? {
            self.send_cmd(acc, Cmd::SyncFolder(folder_id))?;
        }
        Ok(())
    }

    // ---------- accounts ----------

    pub fn accounts(&self) -> Result<Vec<Account>> {
        self.db.accounts()
    }

    pub async fn test_account(&self, acc: &Account, password: Option<String>) -> Result<()> {
        let pw = match password.filter(|p| !p.is_empty()) {
            Some(p) => p,
            None => secrets::get(&acc.email)?,
        };
        let mut s = imap::connect(&acc.imap_host, acc.imap_port, &acc.email, &pw).await?;
        let _ = s.logout().await;
        smtp::test(acc, &pw).await
    }

    /// Saves settings, stores a new password if given, and restarts that account's sync.
    pub async fn save_account(&self, acc: Account, password: Option<String>) -> Result<Account> {
        let mut password = password.filter(|p| !p.is_empty());
        let is_new = acc.id == 0;
        if is_new && password.is_none() {
            return Err(anyhow!("Password required"));
        }
        // Reject duplicate addresses before touching the Keychain so a failed
        // UNIQUE constraint can't overwrite another account's stored password.
        if self.db.accounts()?.iter().any(|a| a.id != acc.id && a.email.eq_ignore_ascii_case(&acc.email)) {
            return Err(anyhow!("An account for {} already exists", acc.email));
        }
        let old = if is_new { None } else { Some(self.db.account(acc.id)?) };
        let old_email = old.as_ref().map(|o| o.email.clone()).filter(|e| *e != acc.email);
        let connection_changed = match &old {
            None => true,
            Some(o) => {
                o.email != acc.email
                    || o.imap_host != acc.imap_host
                    || o.imap_port != acc.imap_port
                    || o.smtp_host != acc.smtp_host
                    || o.smtp_port != acc.smtp_port
            }
        };
        // The Keychain entry is keyed by email; carry it over when the address changes.
        if password.is_none() {
            if let Some(prev) = &old_email {
                password = Some(secrets::get(prev)?);
            }
        }
        if password.is_some() || connection_changed {
            self.test_account(&acc, password.clone()).await?;
        }
        if let Some(pw) = &password {
            secrets::set(&acc.email, pw)?;
        }
        let id = self.db.save_account(&acc)?;
        if let Some(prev) = old_email {
            secrets::delete(&prev);
        }
        let saved = self.db.account(id)?;
        if password.is_some() || connection_changed {
            self.workers.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
            self.start_account(saved.clone());
        }
        Ok(saved)
    }

    pub fn reorder_accounts(&self, ids: &[i64]) -> Result<()> {
        self.db.reorder_accounts(ids)
    }

    pub fn reorder_folders(&self, account_id: i64, ids: &[i64]) -> Result<()> {
        self.db.reorder_folders(account_id, ids)
    }

    pub fn delete_account(&self, id: i64) -> Result<()> {
        let acc = self.db.account(id)?;
        self.workers.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
        self.db.delete_account(id)?;
        secrets::delete(&acc.email);
        Ok(())
    }

    // ---------- reading ----------

    pub fn folders(&self) -> Result<Vec<Folder>> {
        self.db.folders()
    }

    /// A page of a view. `threaded` gives one row per conversation.
    pub fn messages(&self, view: &View, query: Option<&str>, offset: i64, limit: i64, threaded: bool) -> Result<Vec<MessageSummary>> {
        match query {
            None if threaded => self.db.list_threads(view, offset, limit),
            _ => self.db.list(view, query, offset, limit),
        }
    }

    /// The whole conversation a message belongs to, oldest first.
    pub fn thread(&self, id: i64) -> Result<Vec<MessageSummary>> {
        self.db.thread(id)
    }

    /// Fetches older mail from the server for the folders behind a view.
    /// Returns how many messages arrived; 0 means the start of every folder was reached.
    pub async fn load_older(&self, view: &View) -> Result<usize> {
        let folders = self.view_folders(view)?;
        let waits = folders.into_iter().filter_map(|(account_id, folder_id)| {
            let (tx, rx) = oneshot::channel();
            self.send_cmd(account_id, Cmd::FetchOlder { folder_id, reply: tx }).ok()?;
            Some(async move {
                match tokio::time::timeout(Duration::from_secs(120), rx).await {
                    Ok(Ok(Ok(n))) => Ok(n),
                    Ok(Ok(Err(e))) => Err(e),
                    _ => Err(anyhow!("Timed out loading older mail")),
                }
            })
        });
        let results = futures::future::join_all(waits).await;
        let mut total = 0;
        let mut first_err = None;
        for r in results {
            match r {
                Ok(n) => total += n,
                Err(e) => first_err = first_err.or(Some(e)),
            }
        }
        match (total, first_err) {
            (0, Some(e)) => Err(e),
            _ => Ok(total),
        }
    }

    /// (account, folder) pairs behind a view.
    fn view_folders(&self, view: &View) -> Result<Vec<(i64, i64)>> {
        Ok(match view {
            View::Unified { role } => self
                .db
                .folders()?
                .into_iter()
                .filter(|f| f.role.as_deref() == Some(role.as_str()))
                .map(|f| (f.account_id, f.id))
                .collect(),
            View::Folder { folder_id } => vec![(self.db.folder(*folder_id)?.0, *folder_id)],
        })
    }

    pub fn search(&self, query: &str, scope: &SearchScope) -> Result<Vec<MessageSummary>> {
        self.db.search(query, scope, 300)
    }

    /// Asks the servers behind a scope, which also finds mail never downloaded and words in
    /// bodies not cached yet. Hits are stored locally; returns them newest first.
    pub async fn search_server(&self, query: &str, scope: &SearchScope) -> Result<Vec<MessageSummary>> {
        let terms: Vec<String> = query.split_whitespace().map(|t| t.trim_matches('"').to_string()).filter(|t| !t.is_empty()).collect();
        if terms.is_empty() {
            return Ok(vec![]);
        }
        let accounts: Vec<i64> = self.workers.lock().unwrap_or_else(|e| e.into_inner()).keys().copied().collect();
        // Each account with the folders to search there (None: wherever the mail could be).
        let targets: Vec<(i64, Option<Vec<i64>>)> = match scope {
            SearchScope::All => accounts.into_iter().map(|a| (a, None)).collect(),
            SearchScope::Account { account_id } => accounts.into_iter().filter(|a| a == account_id).map(|a| (a, None)).collect(),
            SearchScope::Unified { role } => self.role_targets(role)?,
            SearchScope::Folder { folder_id } => vec![(self.db.folder(*folder_id)?.0, Some(vec![*folder_id]))],
        };
        let ids = self.ask_servers(&imap::search_criteria(&terms), targets).await?;
        let mut rows = self.db.summaries(&ids)?;
        rows.sort_by(|a, b| b.date.cmp(&a.date).then(b.id.cmp(&a.id)));
        rows.dedup_by_key(|r| r.id);
        Ok(rows)
    }

    /// Each account with the folders of a standard role there.
    fn role_targets(&self, role: &str) -> Result<Vec<(i64, Option<Vec<i64>>)>> {
        let mut by_account: HashMap<i64, Vec<i64>> = HashMap::new();
        for f in self.db.folders()?.into_iter().filter(|f| f.role.as_deref() == Some(role)) {
            by_account.entry(f.account_id).or_default().push(f.id);
        }
        Ok(by_account.into_iter().map(|(a, f)| (a, Some(f))).collect())
    }

    /// Runs a SEARCH on each target account (in its folders, or wherever the mail could be).
    /// Hits not cached yet are stored. Fails only when every server failed.
    async fn ask_servers(&self, criteria: &str, targets: Vec<(i64, Option<Vec<i64>>)>) -> Result<Vec<i64>> {
        let waits = targets.into_iter().filter_map(|(acc, folders)| {
            let (tx, rx) = oneshot::channel();
            self.send_cmd(acc, Cmd::Search { criteria: criteria.to_string(), folders, reply: tx }).ok()?;
            Some(async move {
                match tokio::time::timeout(Duration::from_secs(60), rx).await {
                    Ok(Ok(r)) => r,
                    _ => Err(anyhow!("Timed out searching the server")),
                }
            })
        });
        let mut ids = Vec::new();
        let mut first_err = None;
        for r in futures::future::join_all(waits).await {
            match r {
                Ok(found) => ids.extend(found),
                Err(e) => first_err = first_err.or(Some(e)),
            }
        }
        if ids.is_empty() {
            if let Some(e) = first_err {
                return Err(e);
            }
        }
        Ok(ids)
    }

    /// Structured search over the local cache.
    pub fn query_mail(&self, q: &MailQuery) -> Result<MailQueryResult> {
        self.db.query_mail(q)
    }

    /// Structured search that first asks the mail servers, so mail never downloaded (and words in
    /// bodies not cached yet) is found too. Returns the local query including those hits, and
    /// the server error if every server failed (the local results are still returned).
    pub async fn query_mail_server(&self, q: &MailQuery) -> Result<(MailQueryResult, Option<String>)> {
        let mut q = q.clone();
        let mut server_error = None;
        if let Some(criteria) = imap::query_criteria(&q) {
            let accounts: Vec<i64> = self.workers.lock().unwrap_or_else(|e| e.into_inner()).keys().copied().collect();
            let targets: Vec<(i64, Option<Vec<i64>>)> = if let Some(folder_id) = q.folder_id {
                vec![(self.db.folder(folder_id)?.0, Some(vec![folder_id]))]
            } else if let Some(role) = q.role.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_lowercase) {
                self.role_targets(&role)?.into_iter().filter(|(a, _)| q.account_id.is_none_or(|x| x == *a)).collect()
            } else {
                accounts.into_iter().filter(|a| q.account_id.is_none_or(|x| x == *a)).map(|a| (a, None)).collect()
            };
            match self.ask_servers(&criteria, targets).await {
                Ok(ids) => q.server_hits = ids,
                Err(e) => server_error = Some(format!("{e:#}")),
            }
        }
        Ok((self.db.query_mail(&q)?, server_error))
    }

    /// Counts matching messages per sender, domain, month, year, folder or account.
    pub fn group_mail(&self, q: &MailQuery, by: &str) -> Result<(i64, Vec<MailGroup>)> {
        self.db.group_mail(q, by)
    }

    pub fn summaries(&self, ids: &[i64]) -> Result<Vec<MessageSummary>> {
        self.db.summaries(ids)
    }

    /// The text (or image) inside an attachment, for the AI tools.
    pub async fn attachment_content(&self, id: i64, index: usize, offset: usize, max_chars: usize) -> Result<AttachmentContent> {
        let raw = self.raw(id).await?;
        tokio::task::spawn_blocking(move || crate::extract::attachment_content(&raw, index, offset, max_chars))
            .await
            .map_err(|_| anyhow!("Reading the attachment failed"))?
    }

    /// The Dock badge number, per the "badge_mode" setting:
    /// "new" (default) counts unread inbox mail that arrived since the app was last focused,
    /// so mail you mark unread to handle later doesn't badge; "all" counts all unread inbox
    /// mail; "off" shows nothing.
    pub fn badge_count(&self) -> Result<i64> {
        match self.db.setting(BADGE_MODE)?.as_deref() {
            Some("off") => Ok(0),
            Some("all") => self.db.unified_unread(),
            _ => {
                let max = self.db.max_message_id()?;
                let seen = self.db.setting(BADGE_SEEN)?.and_then(|s| s.parse::<i64>().ok());
                // No mark yet (first run): count only mail from here on. Ids can be reused
                // after the newest row is deleted, so pull the mark back down to match.
                let seen = match seen {
                    Some(s) if s <= max => s,
                    _ => {
                        self.db.set_setting(BADGE_SEEN, &max.to_string())?;
                        max
                    }
                };
                self.db.unified_unread_after(seen)
            }
        }
    }

    /// Records that the user has looked at the app: everything stored so far stops being "new".
    pub fn mark_badge_seen(&self) -> Result<()> {
        let max = self.db.max_message_id()?;
        self.db.set_setting(BADGE_SEEN, &max.to_string())
    }

    async fn raw(&self, id: i64) -> Result<Vec<u8>> {
        if let Some(raw) = self.db.body(id)? {
            return Ok(raw);
        }
        let (account_id, folder_id, _, uid) = self.db.locate(id)?;
        let (tx, rx) = oneshot::channel();
        self.send_cmd(account_id, Cmd::FetchBody { folder_id, uid, reply: tx })?;
        tokio::time::timeout(Duration::from_secs(90), rx)
            .await
            .map_err(|_| anyhow!("Timed out loading message"))?
            .map_err(|_| anyhow!("Account worker stopped"))??;
        self.db.body(id)?.ok_or_else(|| anyhow!("Message unavailable"))
    }

    pub async fn message(&self, id: i64) -> Result<MessageDetail> {
        let raw = self.raw(id).await?;
        let (summary, to, cc, reply_to, message_id, references, in_reply_to) = self.db.summary(id)?;
        let p = parse::display(&raw);
        Ok(MessageDetail {
            summary,
            to,
            cc,
            bcc: p.bcc,
            reply_to,
            message_id,
            in_reply_to,
            references,
            html: p.html,
            text: p.text,
            attachments: p.attachments,
            list_id: self.db.list_id(id)?,
        })
    }

    pub async fn attachment(&self, id: i64, index: usize) -> Result<(String, Vec<u8>)> {
        let raw = self.raw(id).await?;
        parse::attachment(&raw, index).ok_or_else(|| anyhow!("Attachment not found"))
    }

    pub async fn all_attachments(&self, id: i64) -> Result<Vec<(String, Vec<u8>)>> {
        let raw = self.raw(id).await?;
        Ok(parse::all_attachments(&raw))
    }

    pub fn contacts(&self, q: &str) -> Result<Vec<Addr>> {
        self.db.contacts(q)
    }

    // ---------- changes (local first, server in background) ----------

    fn group(&self, ids: &[i64]) -> Result<HashMap<(i64, i64), Vec<u32>>> {
        Ok(self.group_ids(ids)?.into_iter().map(|(k, v)| (k, v.into_iter().map(|(_, uid)| uid).collect())).collect())
    }

    /// Messages by (account, folder), as (id, uid).
    fn group_ids(&self, ids: &[i64]) -> Result<GroupedIds> {
        let mut g: HashMap<(i64, i64), Vec<(i64, u32)>> = HashMap::new();
        for id in ids {
            let (acc, folder, _, uid) = self.db.locate(*id)?;
            g.entry((acc, folder)).or_default().push((*id, uid));
        }
        Ok(g)
    }

    pub fn set_seen(&self, ids: &[i64], seen: bool) -> Result<()> {
        let g = self.group(ids)?;
        self.db.set_seen(ids, seen)?;
        for ((acc, folder_id), uids) in g {
            self.send_cmd(acc, Cmd::Store { folder_id, uids, flag: "\\Seen", add: seen })?;
        }
        self.notify_changed(ids);
        Ok(())
    }

    /// Marks everything in the view's folders as read, including mail not cached locally.
    pub fn mark_read(&self, view: &View) -> Result<()> {
        let mut accounts = Vec::new();
        for (acc, folder_id) in self.view_folders(view)? {
            self.db.mark_folder_read(folder_id)?;
            self.send_cmd(acc, Cmd::MarkAllRead { folder_id })?;
            if !accounts.contains(&acc) {
                accounts.push(acc);
            }
        }
        for account_id in accounts {
            let _ = self.events.send(Event::Changed { account_id });
        }
        Ok(())
    }

    pub fn set_flagged(&self, ids: &[i64], flagged: bool) -> Result<()> {
        let g = self.group(ids)?;
        self.db.set_flagged(ids, flagged)?;
        for ((acc, folder_id), uids) in g {
            self.send_cmd(acc, Cmd::Store { folder_id, uids, flag: "\\Flagged", add: flagged })?;
        }
        self.notify_changed(ids);
        Ok(())
    }

    fn move_role(&self, ids: &[i64], role: &'static str) -> Result<()> {
        let g = self.group_ids(ids)?;
        let mut moved = Vec::new();
        let mut accounts = Vec::new();
        for ((acc, folder_id), msgs) in g {
            let (_, _, from_role) = self.db.folder(folder_id)?;
            let uids = msgs.iter().map(|m| m.1).collect();
            if from_role.as_deref() == Some(role) {
                if role == "trash" {
                    self.send_cmd(acc, Cmd::Delete { folder_id, uids })?;
                } else {
                    continue;
                }
            } else {
                self.send_cmd(acc, Cmd::Move { folder_id, uids, to: Target::Role(role) })?;
            }
            moved.extend(msgs.iter().map(|m| m.0));
            if !accounts.contains(&acc) {
                accounts.push(acc);
            }
        }
        self.db.delete_messages(&moved)?;
        for account_id in accounts {
            let _ = self.events.send(Event::Changed { account_id });
        }
        Ok(())
    }

    pub fn archive(&self, ids: &[i64]) -> Result<()> {
        self.move_role(ids, "archive")
    }

    pub fn trash(&self, ids: &[i64]) -> Result<()> {
        self.move_role(ids, "trash")
    }

    /// Moves to the Junk folder. On Gmail, iCloud, Fastmail and others this also trains their spam filter.
    pub fn junk(&self, ids: &[i64]) -> Result<()> {
        self.move_role(ids, "junk")
    }

    /// "Not Junk": moves messages back to the inbox.
    pub fn not_junk(&self, ids: &[i64]) -> Result<()> {
        self.move_role(ids, "inbox")
    }

    /// Moves messages to a folder of their own account.
    pub fn move_to(&self, ids: &[i64], folder_id: i64) -> Result<()> {
        let (target_acc, _, _) = self.db.folder(folder_id)?;
        let g = self.group_ids(ids)?;
        if g.keys().any(|(acc, _)| *acc != target_acc) {
            return Err(anyhow!("Messages can only be moved to a folder in their own account"));
        }
        let mut moved = Vec::new();
        for ((acc, from), msgs) in g {
            if from == folder_id {
                continue;
            }
            let uids = msgs.iter().map(|m| m.1).collect();
            self.send_cmd(acc, Cmd::Move { folder_id: from, uids, to: Target::Folder(folder_id) })?;
            moved.extend(msgs.iter().map(|m| m.0));
        }
        self.db.delete_messages(&moved)?;
        let _ = self.events.send(Event::Changed { account_id: target_acc });
        Ok(())
    }

    /// Takes messages out of their folder until `until`, then brings them back to the
    /// Inbox as unread. They wait in a "Snoozed" folder on the server, so other devices
    /// don't show them either.
    pub fn snooze(&self, ids: &[i64], until: i64) -> Result<()> {
        if until <= now() {
            return Err(anyhow!("Pick a time in the future"));
        }
        let g = self.group_ids(ids)?;
        let mut moved = Vec::new();
        let mut accounts = Vec::new();
        for ((acc, folder_id), msgs) in g {
            let (_, path, role) = self.db.folder(folder_id)?;
            let back_to = self.db.folder_by_role(acc, "inbox")?.map(|f| f.1).unwrap_or(path);
            for (id, _) in &msgs {
                let mid = self.db.message_id_of(*id)?.ok_or_else(|| anyhow!("This message can't be snoozed (it has no Message-ID)"))?;
                self.db.add_snooze(acc, &mid, &back_to, until)?;
            }
            if role.as_deref() != Some("snoozed") {
                let uids = msgs.iter().map(|m| m.1).collect();
                self.send_cmd(acc, Cmd::Move { folder_id, uids, to: Target::Role("snoozed") })?;
                moved.extend(msgs.iter().map(|m| m.0));
            }
            if !accounts.contains(&acc) {
                accounts.push(acc);
            }
        }
        self.db.delete_messages(&moved)?;
        self.wake.notify_one();
        for account_id in accounts {
            let _ = self.events.send(Event::Changed { account_id });
        }
        Ok(())
    }

    /// Brings snoozed messages back now.
    pub fn unsnooze(&self, ids: &[i64]) -> Result<()> {
        let t = now();
        for id in ids {
            let (acc, ..) = self.db.locate(*id)?;
            let Some(mid) = self.db.message_id_of(*id)? else { continue };
            if !self.db.set_wake(acc, &mid, t)? {
                // Snoozed by another app: send it to the Inbox.
                let inbox = self.db.folder_by_role(acc, "inbox")?.map(|f| f.1).unwrap_or_else(|| "INBOX".into());
                self.db.add_snooze(acc, &mid, &inbox, t)?;
            }
        }
        self.wake.notify_one();
        Ok(())
    }

    fn notify_changed(&self, ids: &[i64]) {
        let mut accounts = Vec::new();
        for id in ids {
            if let Ok((acc, ..)) = self.db.locate(*id) {
                if !accounts.contains(&acc) {
                    accounts.push(acc);
                }
            }
        }
        for account_id in accounts {
            let _ = self.events.send(Event::Changed { account_id });
        }
    }

    // ---------- sending ----------

    /// Queues a message in the outbox: for `send_at`, or after the undo window.
    /// It is built now, so a bad address or missing file shows up in the composer.
    /// Returns the outbox id, for undo.
    pub fn send(&self, mut out: Outgoing, send_at: Option<i64>) -> Result<i64> {
        if out.to.is_empty() && out.cc.is_empty() && out.bcc.is_empty() {
            return Err(anyhow!("Add a recipient"));
        }
        let acc = self.db.account(out.account_id)?;
        let existing_mid = out.message_id.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(String::from);
        let was_draft = existing_mid.is_some();
        let message_id = existing_mid.unwrap_or_else(|| smtp::new_message_id(&acc));
        out.message_id = Some(message_id.clone());
        let built = smtp::build(&acc, &out, &message_id, false)?;
        let at = match send_at {
            Some(t) => t,
            None => now() + self.undo_seconds(),
        };
        let id = self.db.add_outbox(
            acc.id,
            &message_id,
            &serde_json::to_string(&out)?,
            &built.raw,
            &serde_json::to_string(&built.envelope)?,
            at,
        )?;
        // It lives in the outbox now; a draft of it would be a second copy.
        if was_draft {
            let _ = self.delete_draft(acc.id, &message_id);
        }
        self.wake.notify_one();
        let _ = self.events.send(Event::Outbox);
        Ok(id)
    }

    fn undo_seconds(&self) -> i64 {
        self.db
            .setting(UNDO_SEND)
            .ok()
            .flatten()
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(UNDO_SEND_DEFAULT)
            .clamp(0, 120)
    }

    /// Takes a message back out of the outbox (undo send, or editing a scheduled one).
    /// Returns it with its attachments, to reopen in the composer.
    pub fn cancel_send(&self, id: i64) -> Result<CancelledSend> {
        let row = self.db.take_outbox(id)?.ok_or_else(|| anyhow!("Too late, it's on its way"))?;
        let out: Outgoing = serde_json::from_str(&row.payload)?;
        let files = parse::all_attachments(&row.raw);
        let _ = self.events.send(Event::Outbox);
        Ok((out, files))
    }

    pub fn send_now(&self, id: i64) -> Result<()> {
        self.db.reschedule_outbox(id, now())?;
        self.wake.notify_one();
        Ok(())
    }

    pub fn outbox(&self) -> Result<Vec<OutboxItem>> {
        Ok(self
            .db
            .outbox()?
            .into_iter()
            .filter_map(|(id, account_id, payload, send_at, error)| {
                let message = serde_json::from_str(&payload).ok()?;
                Some(OutboxItem { id, account_id, message, send_at, error })
            })
            .collect())
    }

    /// Saves a draft to the account's Drafts folder, replacing its previous version.
    /// Returns the draft's Message-ID, to pass back on the next save.
    pub async fn save_draft(&self, mut out: Outgoing) -> Result<String> {
        let acc = self.db.account(out.account_id)?;
        let message_id = out
            .message_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .unwrap_or_else(|| smtp::new_message_id(&acc));
        out.message_id = Some(message_id.clone());
        let built = smtp::build(&acc, &out, &message_id, true)?;
        let (tx, rx) = oneshot::channel();
        self.send_cmd(acc.id, Cmd::SaveDraft { message_id: message_id.clone(), raw: built.raw, reply: tx })?;
        tokio::time::timeout(Duration::from_secs(60), rx)
            .await
            .map_err(|_| anyhow!("Timed out saving the draft"))?
            .map_err(|_| anyhow!("Account worker stopped"))??;
        Ok(message_id)
    }

    /// Finds a draft in any account's Drafts folder by its Message-ID ("<…>"; the brackets
    /// may be left out): (message id, account id).
    pub fn find_draft(&self, message_id: &str) -> Result<Option<(i64, i64)>> {
        let id = message_id.trim();
        if id.is_empty() {
            return Ok(None);
        }
        let id = if id.starts_with('<') { id.to_string() } else { format!("<{id}>") };
        self.db.find_draft(&id)
    }

    pub fn delete_draft(&self, account_id: i64, message_id: &str) -> Result<()> {
        let message_id = message_id.trim();
        if message_id.is_empty() {
            return Ok(());
        }
        if let Some((folder_id, _)) = self.db.folder_by_role(account_id, "drafts")? {
            if self.db.delete_by_message_id(folder_id, message_id)? > 0 {
                let _ = self.events.send(Event::Changed { account_id });
            }
        }
        self.send_cmd(account_id, Cmd::DeleteDraft { message_id: message_id.to_string() })
    }

    // ---------- settings ----------

    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        self.db.setting(key)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.db.set_setting(key, value)
    }

    pub fn delete_setting(&self, key: &str) -> Result<()> {
        self.db.delete_setting(key)
    }

    // ---------- sender pictures ----------

    /// The picture for a sender: their domain's BIMI logo or website icon. None for
    /// personal mail providers, domains without one, and when the setting is off.
    ///
    /// Served from the database; a domain is looked up on first sight and again once its
    /// entry is due. A known picture comes back at once and is refreshed in the background.
    pub async fn sender_icon(&self, addr: &str) -> Result<Option<Icon>> {
        let Some(domain) = icons::icon_domain(addr) else { return Ok(None) };
        if self.db.setting(SENDER_ICONS)?.as_deref() == Some("off") {
            return Ok(None);
        }
        let cached = self.db.sender_icon(&domain)?;
        if let Some((icon, retry_at)) = &cached {
            if *retry_at > now() {
                return Ok(icon.clone());
            }
        }
        let Some(fetcher) = self.icons.get_or_init(|| icons::Fetcher::new().ok().map(Arc::new)).clone() else {
            return Ok(cached.and_then(|c| c.0));
        };
        let old = cached.and_then(|c| c.0);
        // Hold back a second lookup of the same domain while this one runs.
        self.db.set_sender_icon(&domain, old.as_ref(), now() + 60)?;
        let db = self.db.clone();
        let lookup = {
            let old = old.clone();
            async move {
                let (icon, wait) = match fetcher.fetch(&domain).await {
                    Outcome::Found(icon) => (Some(icon), 30 * 86_400),
                    Outcome::Missing => (None, 14 * 86_400),
                    Outcome::Failed => (old, 6 * 3_600),
                };
                let _ = db.set_sender_icon(&domain, icon.as_ref(), now() + wait);
                icon
            }
        };
        if old.is_some() {
            tokio::spawn(lookup);
            Ok(old)
        } else {
            Ok(lookup.await)
        }
    }
}

/// Sends outbox messages and wakes snoozes when they come due.
async fn scheduler(db: Arc<Db>, events: broadcast::Sender<Event>, workers: Workers, wake: Arc<Notify>) {
    let _ = db.reset_outbox_claims();
    loop {
        let t = now();
        match db.claim_due_outbox(t) {
            Ok(rows) => {
                for row in rows {
                    tokio::spawn(deliver(db.clone(), events.clone(), workers.clone(), row));
                }
            }
            Err(e) => eprintln!("outbox: {e:#}"),
        }
        if let Ok(due) = db.due_snoozes(t) {
            for s in due {
                // Hold it back while the move runs; if that fails it's retried then.
                if let Err(e) = db.set_wake(s.account_id, &s.message_id, t.saturating_add(120)) {
                    eprintln!("snooze: {e:#}");
                    continue;
                }
                tokio::spawn(wake_snooze(db.clone(), events.clone(), workers.clone(), s));
            }
        }
        let next_ms = db.next_due().ok().flatten().map(|n| n.saturating_mul(1000));
        let now_ms = chrono::Utc::now().timestamp_millis();
        // Keep a small floor so a past-due row that failed to claim can't spin the loop;
        // explicit schedules / send_now wake immediately via `wake.notify_one()`.
        let wait = next_ms.map(|n| n.saturating_sub(now_ms).clamp(250, 60_000)).unwrap_or(60_000);
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(wait as u64)) => {}
            _ = wake.notified() => {}
        }
    }
}

async fn deliver(db: Arc<Db>, events: broadcast::Sender<Event>, workers: Workers, row: OutboxRow) {
    let res = async {
        let acc = db.account(row.account_id)?;
        let pw = secrets::get(&acc.email)?;
        let envelope: smtp::SimpleEnvelope = serde_json::from_str(&row.envelope)?;
        let raw = smtp::redate(&row.raw, chrono::Local::now());
        smtp::send_raw(&acc, &pw, &envelope, &raw).await?;
        Ok::<_, anyhow::Error>(raw)
    }
    .await;
    match res {
        Ok(raw) => {
            let _ = db.delete_outbox(row.id);
            let _ = events.send(Event::Outbox);
            if let Some(tx) = worker_tx(&workers, row.account_id) {
                // Most servers file SMTP mail into Sent themselves. Give them a moment,
                // then append a copy only if it didn't show up.
                let message_id = row.message_id;
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    let _ = tx.send(Cmd::EnsureSent { message_id, raw });
                });
            }
        }
        Err(e) => {
            let message = format!("{e:#}");
            let backoff = 60 * (1i64 << row.attempts.min(10));
            let _ = db.outbox_failed(row.id, &message, now().saturating_add(backoff));
            if row.attempts == 0 {
                let subject = serde_json::from_str::<Outgoing>(&row.payload).map(|o| o.subject).unwrap_or_default();
                let _ = events.send(Event::SendFailed { subject, message });
            }
            let _ = events.send(Event::Outbox);
        }
    }
}

async fn wake_snooze(db: Arc<Db>, events: broadcast::Sender<Event>, workers: Workers, s: DueSnooze) {
    let Some(tx) = worker_tx(&workers, s.account_id) else { return };
    let (rtx, rrx) = oneshot::channel();
    if tx.send(Cmd::Unsnooze { message_id: s.message_id.clone(), to_path: s.return_path.clone(), reply: rtx }).is_err() {
        return;
    }
    // Back in the Inbox as unread, it notifies like any new mail.
    if let Ok(Ok(Ok(_))) = tokio::time::timeout(Duration::from_secs(90), rrx).await {
        let _ = db.mark_woke(s.id, now());
        let _ = events.send(Event::Changed { account_id: s.account_id });
    }
}
