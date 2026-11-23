//! One worker per account. It owns the IMAP connection and processes commands in order.
//! A second connection sits in IDLE on the inbox and pokes the worker when mail arrives.

use crate::db::{Db, NewMessage};
use crate::imap::{self, Session};
use crate::model::{Account, Event};
use crate::{parse, secrets};
use anyhow::{anyhow, Result};
use async_imap::extensions::idle::IdleResponse;
use async_imap::types::Fetch;
use futures::TryStreamExt;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, oneshot};

const INITIAL_HEADERS: u32 = 500;
const HEADER_ITEMS: &str = "(UID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[HEADER])";
const POLL_EVERY: Duration = Duration::from_secs(180);
/// How many older headers to fetch each time the list reaches the end of the local cache.
const BACKFILL: usize = 500;

pub enum Cmd {
    SyncAll,
    SyncFolder(i64),
    FetchBody { folder_id: i64, uid: u32, reply: oneshot::Sender<Result<()>> },
    /// Download headers older than anything held locally. Replies with how many arrived (0 = none left).
    FetchOlder { folder_id: i64, reply: oneshot::Sender<Result<usize>> },
    Store { folder_id: i64, uids: Vec<u32>, flag: &'static str, add: bool },
    /// Set \Seen on every unread message in the folder on the server.
    MarkAllRead { folder_id: i64 },
    Move { folder_id: i64, uids: Vec<u32>, to_role: &'static str },
    Delete { folder_id: i64, uids: Vec<u32> },
    EnsureSent { message_id: String, raw: Vec<u8> },
}

impl Cmd {
    fn is_sync(&self) -> bool {
        matches!(self, Cmd::SyncAll | Cmd::SyncFolder(_))
    }
}

pub struct Worker {
    account: Account,
    db: Arc<Db>,
    events: broadcast::Sender<Event>,
    rx: mpsc::UnboundedReceiver<Cmd>,
    session: Option<Session>,
    selected: Option<String>,
    deferred: VecDeque<Cmd>,
}

impl Worker {
    pub fn new(account: Account, db: Arc<Db>, events: broadcast::Sender<Event>, rx: mpsc::UnboundedReceiver<Cmd>) -> Self {
        Worker { account, db, events, rx, session: None, selected: None, deferred: VecDeque::new() }
    }

    fn emit(&self, e: Event) {
        let _ = self.events.send(e);
    }

    fn status(&self, state: &str, message: Option<String>) {
        self.emit(Event::Status { account_id: self.account.id, state: state.into(), message });
    }

    fn changed(&self) {
        self.emit(Event::Changed { account_id: self.account.id });
    }

    pub async fn run(mut self) {
        let mut ticker = tokio::time::interval(POLL_EVERY);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            let cmd = if let Some(c) = self.deferred.pop_front() {
                c
            } else {
                tokio::select! {
                    c = self.rx.recv() => match c { Some(c) => c, None => break },
                    _ = ticker.tick() => Cmd::SyncAll,
                }
            };
            // Collapse a burst of sync requests into one.
            if cmd.is_sync() && self.deferred.iter().any(|c| matches!(c, Cmd::SyncAll)) {
                continue;
            }
            self.handle(cmd).await;
        }
        if let Some(mut s) = self.session.take() {
            let _ = s.logout().await;
        }
    }

    async fn handle(&mut self, cmd: Cmd) {
        if cmd.is_sync() {
            let had_session = self.session.is_some();
            let mut res = self.exec(&cmd).await;
            if res.is_err() && had_session {
                self.reset();
                res = self.exec(&cmd).await;
            }
            if let Err(e) = res {
                self.fail(e);
            }
        } else {
            self.handle_quick(cmd).await;
        }
    }

    /// Never triggers a full sync, so it is safe to call from inside one.
    async fn handle_quick(&mut self, mut cmd: Cmd) {
        if let Cmd::FetchOlder { folder_id, reply } = cmd {
            let had_session = self.session.is_some();
            let mut res = self.fetch_older(folder_id).await;
            if res.is_err() && had_session {
                self.reset();
                res = self.fetch_older(folder_id).await;
            }
            let res = match res {
                Err(e) => {
                    let msg = format!("{e:#}");
                    self.fail(e);
                    Err(anyhow!(msg))
                }
                ok => ok,
            };
            let _ = reply.send(res);
            return;
        }
        let reply = match &mut cmd {
            Cmd::FetchBody { reply, .. } => {
                let (tx, _) = oneshot::channel();
                Some(std::mem::replace(reply, tx))
            }
            _ => None,
        };
        let had_session = self.session.is_some();
        let mut res = self.exec_quick(&cmd).await;
        if res.is_err() && had_session {
            self.reset();
            res = self.exec_quick(&cmd).await;
        }
        let res = match res {
            Err(e) => {
                let msg = format!("{e:#}");
                self.fail(e);
                Err(anyhow!(msg))
            }
            ok => ok,
        };
        if let Some(r) = reply {
            let _ = r.send(res);
        }
    }

    fn reset(&mut self) {
        self.session = None;
        self.selected = None;
    }

    fn fail(&mut self, e: anyhow::Error) {
        self.reset();
        self.status("error", Some(format!("{e:#}")));
    }

    async fn ensure(&mut self) -> Result<&mut Session> {
        if self.session.is_none() {
            let pw = secrets::get(&self.account.email)?;
            let s = imap::connect(&self.account.imap_host, self.account.imap_port, &self.account.email, &pw).await?;
            self.session = Some(s);
            self.selected = None;
        }
        Ok(self.session.as_mut().unwrap())
    }

    async fn select(&mut self, path: &str) -> Result<()> {
        if self.selected.as_deref() == Some(path) {
            return Ok(());
        }
        let s = self.ensure().await?;
        s.select(path).await?;
        self.selected = Some(path.to_string());
        Ok(())
    }

    async fn exec(&mut self, cmd: &Cmd) -> Result<()> {
        match cmd {
            Cmd::SyncAll => self.sync_all().await,
            Cmd::SyncFolder(id) => {
                let (_, path, role) = self.db.folder(*id)?;
                self.sync_folder(*id, &path, role.as_deref()).await?;
                self.prefetch(*id, &path, role.as_deref()).await?;
                self.status("idle", None);
                Ok(())
            }
            other => self.exec_quick(other).await,
        }
    }

    /// Commands the user is waiting on. Safe to run in the middle of a long sync.
    async fn exec_quick(&mut self, cmd: &Cmd) -> Result<()> {
        match cmd {
            Cmd::FetchBody { folder_id, uid, .. } => {
                let (_, path, _) = self.db.folder(*folder_id)?;
                self.select(&path).await?;
                let s = self.ensure().await?;
                let fetches: Vec<Fetch> = s.uid_fetch(uid.to_string(), "(UID BODY.PEEK[])").await?.try_collect().await?;
                let f = fetches.iter().find(|f| f.uid == Some(*uid)).ok_or_else(|| anyhow!("Message no longer on server"))?;
                let raw = f.body().ok_or_else(|| anyhow!("Empty message"))?;
                self.db.store_body(*folder_id, *uid, raw, &parse::body_info(raw))?;
                Ok(())
            }
            Cmd::Store { folder_id, uids, flag, add } => {
                let (_, path, _) = self.db.folder(*folder_id)?;
                self.select(&path).await?;
                let s = self.ensure().await?;
                let q = format!("{}FLAGS.SILENT ({})", if *add { "+" } else { "-" }, flag);
                let _: Vec<Fetch> = s.uid_store(imap::uid_set(uids), q).await?.try_collect().await?;
                Ok(())
            }
            Cmd::MarkAllRead { folder_id } => {
                let (_, path, _) = self.db.folder(*folder_id)?;
                self.select(&path).await?;
                let s = self.ensure().await?;
                let uids: Vec<u32> = s.uid_search("UNSEEN").await?.into_iter().collect();
                if uids.is_empty() {
                    return Ok(());
                }
                let _: Vec<Fetch> = s.uid_store(imap::uid_set(&uids), "+FLAGS.SILENT (\\Seen)").await?.try_collect().await?;
                Ok(())
            }
            Cmd::Move { folder_id, uids, to_role } => {
                let (to_id, to_path) = self.target_folder(to_role).await?;
                let (_, path, _) = self.db.folder(*folder_id)?;
                self.select(&path).await?;
                let set = imap::uid_set(uids);
                let s = self.ensure().await?;
                if s.uid_mv(&set, &to_path).await.is_err() {
                    s.uid_copy(&set, &to_path).await?;
                    let _: Vec<Fetch> = s.uid_store(&set, "+FLAGS.SILENT (\\Deleted)").await?.try_collect().await?;
                    let _: Vec<u32> = s.expunge().await?.try_collect().await?;
                }
                self.sync_folder(to_id, &to_path, Some(to_role)).await?;
                Ok(())
            }
            Cmd::Delete { folder_id, uids } => {
                let (_, path, _) = self.db.folder(*folder_id)?;
                self.select(&path).await?;
                let set = imap::uid_set(uids);
                let s = self.ensure().await?;
                let _: Vec<Fetch> = s.uid_store(&set, "+FLAGS.SILENT (\\Deleted)").await?.try_collect().await?;
                let _: Vec<u32> = s.expunge().await?.try_collect().await?;
                Ok(())
            }
            Cmd::EnsureSent { message_id, raw } => {
                let Some((sent_id, sent_path)) = self.db.folder_by_role(self.account.id, "sent")? else { return Ok(()) };
                self.sync_folder(sent_id, &sent_path, Some("sent")).await?;
                if !self.db.has_message_id(sent_id, message_id)? {
                    let s = self.ensure().await?;
                    s.append(&sent_path, Some("(\\Seen)"), None, raw).await?;
                    self.sync_folder(sent_id, &sent_path, Some("sent")).await?;
                }
                Ok(())
            }
            Cmd::SyncAll | Cmd::SyncFolder(_) | Cmd::FetchOlder { .. } => Ok(()),
        }
    }

    async fn fetch_older(&mut self, folder_id: i64) -> Result<usize> {
        let (_, path, _) = self.db.folder(folder_id)?;
        let (min_uid, _) = self.db.uid_range(folder_id)?;
        if min_uid <= 1 {
            return Ok(0);
        }
        self.select(&path).await?;
        let s = self.ensure().await?;
        let found = s.uid_search(format!("UID 1:{}", min_uid - 1)).await?;
        let mut uids: Vec<u32> = found.into_iter().filter(|u| *u < min_uid).collect();
        uids.sort_unstable();
        let batch: Vec<u32> = uids.iter().rev().take(BACKFILL).copied().collect();
        if batch.is_empty() {
            return Ok(0);
        }
        let fetches: Vec<Fetch> = s.uid_fetch(imap::uid_set(&batch), HEADER_ITEMS).await?.try_collect().await?;
        let older = headers(&fetches, |uid| uid < min_uid);
        self.db.insert_messages(self.account.id, folder_id, &older)?;
        self.changed();
        Ok(older.len())
    }

    async fn target_folder(&mut self, role: &str) -> Result<(i64, String)> {
        if let Some(f) = self.db.folder_by_role(self.account.id, role)? {
            return Ok(f);
        }
        if role == "archive" {
            if let Some(f) = self.db.folder_by_role(self.account.id, "all")? {
                return Ok(f);
            }
        }
        let name = match role {
            "archive" => "Archive",
            "trash" => "Trash",
            "junk" => "Junk",
            other => return Err(anyhow!("No {other} folder")),
        };
        let s = self.ensure().await?;
        s.create(name).await?;
        let id = self.db.upsert_folder(self.account.id, name, name, Some(role))?;
        Ok((id, name.to_string()))
    }

    /// Runs commands that arrived while a long sync is in progress.
    async fn drain_urgent(&mut self) {
        while let Ok(cmd) = self.rx.try_recv() {
            if cmd.is_sync() {
                self.deferred.push_back(cmd);
            } else {
                self.handle_quick(cmd).await;
            }
        }
    }

    async fn sync_all(&mut self) -> Result<()> {
        self.status("syncing", None);
        let s = self.ensure().await?;
        let remote = imap::list_folders(s).await?;
        let mut folders = Vec::new();
        for f in &remote {
            let id = self.db.upsert_folder(self.account.id, &f.path, &f.name, f.role)?;
            folders.push((id, f.path.clone(), f.role));
        }
        self.db.remove_folders_except(self.account.id, &folders.iter().map(|f| f.0).collect::<Vec<_>>())?;
        let rank = |r: Option<&str>| match r {
            Some("inbox") => 0,
            Some("sent") => 1,
            Some("drafts") => 2,
            Some("archive") => 3,
            None => 4,
            _ => 5,
        };
        folders.sort_by_key(|f| rank(f.2));
        self.changed();

        // Headers first everywhere, so every folder is browsable quickly.
        for (id, path, role) in &folders {
            self.sync_folder(*id, path, *role).await?;
            self.drain_urgent().await;
        }
        for (id, path, role) in &folders {
            self.prefetch(*id, path, *role).await?;
        }
        self.status("idle", None);
        Ok(())
    }

    async fn sync_folder(&mut self, folder_id: i64, path: &str, role: Option<&str>) -> Result<()> {
        let s = self.ensure().await?;
        let mb = s.select(path).await?;
        self.selected = Some(path.to_string());
        let uv = mb.uid_validity.unwrap_or(0);
        let mut changed = false;
        if self.db.folder_uidvalidity(folder_id)? != uv {
            self.db.reset_folder(folder_id, uv)?;
            changed = true;
        }
        if mb.exists == 0 {
            changed |= self.db.reconcile_flags(folder_id, 0, &[])?;
            if changed {
                self.changed();
            }
            return Ok(());
        }

        let (_, max_uid) = self.db.uid_range(folder_id)?;
        let s = self.session.as_mut().unwrap();
        let fetches: Vec<Fetch> = if max_uid == 0 {
            let start = mb.exists.saturating_sub(INITIAL_HEADERS - 1).max(1);
            s.fetch(format!("{start}:*"), HEADER_ITEMS).await?.try_collect().await?
        } else {
            s.uid_fetch(format!("{}:*", max_uid + 1), HEADER_ITEMS).await?.try_collect().await?
        };
        let new = headers(&fetches, |uid| uid > max_uid);

        if !new.is_empty() {
            self.db.insert_messages(self.account.id, folder_id, &new)?;
            changed = true;
            if role == Some("inbox") && max_uid > 0 {
                let unseen: Vec<&NewMessage> = new.iter().filter(|m| !m.seen).collect();
                if let Some(last) = unseen.iter().max_by_key(|m| m.date) {
                    self.emit(Event::NewMail {
                        account_id: self.account.id,
                        from: if last.from_name.is_empty() { last.from_addr.clone() } else { last.from_name.clone() },
                        subject: last.subject.clone(),
                        count: unseen.len(),
                    });
                }
            }
        }

        // Reconcile flags and deletions for everything we hold locally.
        let (min_uid, _) = self.db.uid_range(folder_id)?;
        if min_uid > 0 {
            let s = self.session.as_mut().unwrap();
            let flags: Vec<Fetch> = s.uid_fetch(format!("{min_uid}:*"), "(UID FLAGS)").await?.try_collect().await?;
            let flags: Vec<(u32, bool, bool)> = flags
                .iter()
                .filter_map(|f| {
                    let (seen, flagged) = imap::flags_of(f);
                    Some((f.uid?, seen, flagged))
                })
                .collect();
            changed |= self.db.reconcile_flags(folder_id, min_uid, &flags)?;
        }
        if changed {
            self.changed();
        }
        Ok(())
    }

    /// Downloads recent bodies ahead of time so opening a message is instant.
    async fn prefetch(&mut self, folder_id: i64, path: &str, role: Option<&str>) -> Result<()> {
        let (limit, max_size) = match role {
            Some("inbox") => (300, 8_000_000),
            Some("sent") | Some("archive") | None => (60, 2_000_000),
            _ => (0, 0),
        };
        if limit == 0 {
            return Ok(());
        }
        let uids = self.db.missing_bodies(folder_id, limit, max_size)?;
        for chunk in uids.chunks(15) {
            self.select(path).await?;
            let s = self.ensure().await?;
            let fetches: Vec<Fetch> = s.uid_fetch(imap::uid_set(chunk), "(UID BODY.PEEK[])").await?.try_collect().await?;
            for f in &fetches {
                if let (Some(uid), Some(raw)) = (f.uid, f.body()) {
                    self.db.store_body(folder_id, uid, raw, &parse::body_info(raw))?;
                }
            }
            self.changed();
            self.drain_urgent().await;
        }
        Ok(())
    }
}

fn headers(fetches: &[Fetch], keep: impl Fn(u32) -> bool) -> Vec<NewMessage> {
    fetches
        .iter()
        .filter_map(|f| {
            let uid = f.uid?;
            if !keep(uid) {
                return None;
            }
            let (seen, flagged) = imap::flags_of(f);
            let internal = f.internal_date().map(|d| d.timestamp()).unwrap_or(0);
            Some(parse::parse_header(uid, f.header().unwrap_or(b""), seen, flagged, f.size.unwrap_or(0), internal))
        })
        .collect()
}

/// Holds an IDLE connection on the inbox and asks the worker to sync on any change.
pub async fn idle_loop(account: Account, db: Arc<Db>, tx: mpsc::UnboundedSender<Cmd>) {
    let mut backoff = 5u64;
    loop {
        match idle_once(&account, &db, &tx).await {
            Ok(()) => return, // worker gone
            Err(_) => {
                tokio::time::sleep(Duration::from_secs(backoff)).await;
                backoff = (backoff * 2).min(300);
            }
        }
    }
}

async fn idle_once(account: &Account, db: &Db, tx: &mpsc::UnboundedSender<Cmd>) -> Result<()> {
    let pw = secrets::get(&account.email)?;
    let mut session = imap::connect(&account.imap_host, account.imap_port, &account.email, &pw).await?;
    session.select("INBOX").await?;
    loop {
        let mut idle = session.idle();
        idle.init().await?;
        let res = {
            let (wait, _stop) = idle.wait_with_timeout(Duration::from_secs(20 * 60));
            wait.await?
        };
        session = idle.done().await?;
        if let IdleResponse::NewData(_) = res {
            let cmd = match db.folder_by_role(account.id, "inbox")? {
                Some((id, _)) => Cmd::SyncFolder(id),
                None => Cmd::SyncAll,
            };
            if tx.send(cmd).is_err() {
                return Ok(());
            }
        }
        if tx.is_closed() {
            return Ok(());
        }
    }
}
