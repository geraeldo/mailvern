use crate::db::Db;
use crate::model::*;
use crate::worker::{idle_loop, Cmd, Worker};
use crate::{imap, parse, secrets, smtp};
use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;

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

/// The public API the UI talks to. Every read is served from the local database.
pub struct Engine {
    db: Arc<Db>,
    events: broadcast::Sender<Event>,
    workers: Mutex<HashMap<i64, Handle>>,
}

impl Engine {
    pub fn open(db_path: &Path) -> Result<Self> {
        let (events, _) = broadcast::channel(256);
        Ok(Engine { db: Arc::new(Db::open(db_path)?), events, workers: Mutex::new(HashMap::new()) })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    /// Starts a worker for every account. Must be called inside a Tokio runtime.
    pub fn start(&self) -> Result<()> {
        for acc in self.db.accounts()? {
            self.start_account(acc);
        }
        Ok(())
    }

    fn start_account(&self, acc: Account) {
        let (tx, rx) = mpsc::unbounded_channel();
        let worker = Worker::new(acc.clone(), self.db.clone(), self.events.clone(), rx);
        let _ = tx.send(Cmd::SyncAll);
        let tasks = vec![
            tokio::spawn(worker.run()),
            tokio::spawn(idle_loop(acc.clone(), self.db.clone(), tx.clone())),
        ];
        self.workers.lock().unwrap().insert(acc.id, Handle { tx, tasks });
    }

    fn send_cmd(&self, account_id: i64, cmd: Cmd) -> Result<()> {
        let w = self.workers.lock().unwrap();
        let h = w.get(&account_id).ok_or_else(|| anyhow!("Account is not running"))?;
        h.tx.send(cmd).map_err(|_| anyhow!("Account worker stopped"))
    }

    pub fn sync_now(&self) {
        for h in self.workers.lock().unwrap().values() {
            let _ = h.tx.send(Cmd::SyncAll);
        }
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
            self.workers.lock().unwrap().remove(&id);
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
        self.workers.lock().unwrap().remove(&id);
        self.db.delete_account(id)?;
        secrets::delete(&acc.email);
        Ok(())
    }

    // ---------- reading ----------

    pub fn folders(&self) -> Result<Vec<Folder>> {
        self.db.folders()
    }

    pub fn messages(&self, view: &View, query: Option<&str>, offset: i64, limit: i64) -> Result<Vec<MessageSummary>> {
        self.db.list(view, query, offset, limit)
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

    pub fn search_all(&self, query: &str) -> Result<Vec<MessageSummary>> {
        self.db.search_all(query, 300)
    }

    pub fn unread_count(&self) -> Result<i64> {
        self.db.unified_unread()
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
        let (summary, to, cc, reply_to, message_id, references, _) = self.db.summary(id)?;
        let p = parse::display(&raw);
        Ok(MessageDetail {
            summary,
            to,
            cc,
            reply_to,
            message_id,
            references,
            html: p.html,
            text: p.text,
            attachments: p.attachments,
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
        let mut g: HashMap<(i64, i64), Vec<u32>> = HashMap::new();
        for id in ids {
            let (acc, folder, _, uid) = self.db.locate(*id)?;
            g.entry((acc, folder)).or_default().push(uid);
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
        let g = self.group(ids)?;
        let mut to_delete = Vec::new();
        for ((acc, folder_id), uids) in g {
            let (_, _, from_role) = self.db.folder(folder_id)?;
            if from_role.as_deref() == Some(role) {
                if role == "trash" {
                    self.send_cmd(acc, Cmd::Delete { folder_id, uids })?;
                } else {
                    continue;
                }
            } else {
                self.send_cmd(acc, Cmd::Move { folder_id, uids, to_role: role })?;
            }
            to_delete.push(acc);
        }
        self.db.delete_messages(ids)?;
        for acc in to_delete {
            let _ = self.events.send(Event::Changed { account_id: acc });
        }
        Ok(())
    }

    pub fn archive(&self, ids: &[i64]) -> Result<()> {
        self.move_role(ids, "archive")
    }

    pub fn trash(&self, ids: &[i64]) -> Result<()> {
        self.move_role(ids, "trash")
    }

    fn notify_changed(&self, ids: &[i64]) {
        if let Some(id) = ids.first() {
            if let Ok((acc, ..)) = self.db.locate(*id) {
                let _ = self.events.send(Event::Changed { account_id: acc });
            }
        }
    }

    // ---------- sending ----------

    pub async fn send(&self, out: Outgoing) -> Result<()> {
        if out.to.is_empty() && out.cc.is_empty() && out.bcc.is_empty() {
            return Err(anyhow!("Add a recipient"));
        }
        let acc = self.db.account(out.account_id)?;
        let pw = secrets::get(&acc.email)?;
        let (message_id, raw) = smtp::send(&acc, &pw, &out).await?;
        // Most servers file SMTP mail into Sent themselves. Give them a moment,
        // then append a copy only if it didn't show up.
        let tx = self.workers.lock().unwrap().get(&acc.id).map(|h| h.tx.clone());
        if let Some(tx) = tx {
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(5)).await;
                let _ = tx.send(Cmd::EnsureSent { message_id, raw });
            });
        }
        Ok(())
    }

    // ---------- settings ----------

    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        self.db.setting(key)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.db.set_setting(key, value)
    }
}
