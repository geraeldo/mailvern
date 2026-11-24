//! One worker per account. It owns the IMAP connection and processes commands in order.
//! A second connection sits in IDLE on the inbox and pokes the worker when mail arrives.

use crate::db::{Db, NewMessage};
use crate::imap::{self, Session, CMD_TIMEOUT, FETCH_TIMEOUT};
use crate::model::{Account, Event};
use crate::{parse, secrets};
use anyhow::{anyhow, Result};
use async_imap::extensions::idle::IdleResponse;
use async_imap::types::Fetch;
use futures::TryStreamExt;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, oneshot, Notify};

const INITIAL_HEADERS: u32 = 500;
const HEADER_ITEMS: &str = "(UID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[HEADER])";
const POLL_EVERY: Duration = Duration::from_secs(180);
/// How many older headers to fetch each time the list reaches the end of the local cache.
const BACKFILL: usize = 500;

/// Where a move goes: a standard folder (created if missing) or a specific folder.
pub enum Target {
    Role(&'static str),
    Folder(i64),
}

/// Most hits fetched per folder by a server search.
const SEARCH_HITS: usize = 50;

/// Maximum UIDs sent in a single IMAP command so fragmented sets stay well under line limits.
const UID_CHUNK: usize = 200;

async fn timed<T, E: Into<anyhow::Error>>(dur: Duration, label: &str, fut: impl Future<Output = std::result::Result<T, E>>) -> Result<T> {
    tokio::time::timeout(dur, fut)
        .await
        .map_err(|_| anyhow!("Timed out {label}"))?
        .map_err(Into::into)
}

type BoxFut<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

pub enum Cmd {
    SyncAll,
    SyncFolder(i64),
    FetchBody { folder_id: i64, uid: u32, reply: oneshot::Sender<Result<()>> },
    /// Download headers older than anything held locally. Replies with how many arrived (0 = none left).
    FetchOlder { folder_id: i64, reply: oneshot::Sender<Result<usize>> },
    Store { folder_id: i64, uids: Vec<u32>, flag: &'static str, add: bool },
    /// Set \Seen on every unread message in the folder on the server.
    MarkAllRead { folder_id: i64 },
    Move { folder_id: i64, uids: Vec<u32>, to: Target },
    Delete { folder_id: i64, uids: Vec<u32> },
    EnsureSent { message_id: String, raw: Vec<u8> },
    /// Stores a draft in Drafts, replacing any earlier version with the same Message-ID.
    SaveDraft { message_id: String, raw: Vec<u8>, reply: oneshot::Sender<Result<()>> },
    /// Removes a draft (after sending it, or when it's discarded).
    DeleteDraft { message_id: String },
    /// Moves a snoozed message back to `to_path` as unread. Replies false if it's no longer in Snoozed.
    Unsnooze { message_id: String, to_path: String, reply: oneshot::Sender<Result<bool>> },
    /// Searches the server, in `folders` or wherever the mail could be.
    /// Replies with the local ids of the hits (stored if they weren't).
    Search { criteria: String, folders: Option<Vec<i64>>, reply: oneshot::Sender<Result<Vec<i64>>> },
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
    /// Wakes the rule runner after new Inbox mail is queued for it.
    rules_wake: Arc<Notify>,
    rx: mpsc::UnboundedReceiver<Cmd>,
    session: Option<Session>,
    selected: Option<String>,
    deferred: VecDeque<Cmd>,
}

impl Worker {
    pub fn new(account: Account, db: Arc<Db>, events: broadcast::Sender<Event>, rules_wake: Arc<Notify>, rx: mpsc::UnboundedReceiver<Cmd>) -> Self {
        Worker { account, db, events, rules_wake, rx, session: None, selected: None, deferred: VecDeque::new() }
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

    /// Runs `f`, once more on a fresh connection if it fails on a reused one.
    /// `report` shows a failure as the account's status; a search failure shouldn't.
    async fn retry<T>(&mut self, report: bool, f: impl for<'a> Fn(&'a mut Worker) -> BoxFut<'a, T>) -> Result<T> {
        let had_session = self.session.is_some();
        let mut res = f(self).await;
        if res.is_err() && had_session {
            self.reset();
            res = f(self).await;
        }
        match res {
            Err(e) => {
                let msg = format!("{e:#}");
                if report {
                    self.fail(e);
                } else {
                    self.reset();
                }
                Err(anyhow!(msg))
            }
            ok => ok,
        }
    }

    /// Never triggers a full sync, so it is safe to call from inside one.
    async fn handle_quick(&mut self, mut cmd: Cmd) {
        match cmd {
            Cmd::FetchOlder { folder_id, reply } => {
                let res = self.retry(true, |w| Box::pin(w.fetch_older(folder_id))).await;
                let _ = reply.send(res);
                return;
            }
            Cmd::SaveDraft { message_id, raw, reply } => {
                let res = self
                    .retry(true, |w| {
                        let (m, r) = (message_id.clone(), raw.clone());
                        Box::pin(async move { w.save_draft(&m, &r).await })
                    })
                    .await;
                let _ = reply.send(res);
                return;
            }
            Cmd::Unsnooze { message_id, to_path, reply } => {
                let res = self
                    .retry(true, |w| {
                        let (m, p) = (message_id.clone(), to_path.clone());
                        Box::pin(async move { w.unsnooze(&m, &p).await })
                    })
                    .await;
                let _ = reply.send(res);
                return;
            }
            Cmd::Search { criteria, folders, reply } => {
                let res = self
                    .retry(false, |w| {
                        let (t, f) = (criteria.clone(), folders.clone());
                        Box::pin(async move { w.search(&t, f.as_deref()).await })
                    })
                    .await;
                let _ = reply.send(res);
                return;
            }
            _ => {}
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
        imap::select(s, path).await?;
        self.selected = Some(path.to_string());
        Ok(())
    }

    async fn exec(&mut self, cmd: &Cmd) -> Result<()> {
        match cmd {
            Cmd::SyncAll => self.sync_all().await,
            Cmd::SyncFolder(id) => {
                self.status("syncing", None);
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
                let fetches: Vec<Fetch> = timed(FETCH_TIMEOUT, "loading message", async {
                    s.uid_fetch(uid.to_string(), "(UID BODY.PEEK[])").await?.try_collect().await
                })
                .await?;
                let f = fetches.iter().find(|f| f.uid == Some(*uid)).ok_or_else(|| anyhow!("Message no longer on server"))?;
                let raw = f.body().ok_or_else(|| anyhow!("Empty message"))?;
                self.db.store_body(*folder_id, *uid, raw, &parse::body_info(raw))?;
                Ok(())
            }
            Cmd::Store { folder_id, uids, flag, add } => {
                if uids.is_empty() {
                    return Ok(());
                }
                let (_, path, _) = self.db.folder(*folder_id)?;
                self.select(&path).await?;
                let q = format!("{}FLAGS.SILENT ({})", if *add { "+" } else { "-" }, flag);
                for chunk in uids.chunks(UID_CHUNK) {
                    let s = self.ensure().await?;
                    let _: Vec<Fetch> = timed(CMD_TIMEOUT, "updating flags", async {
                        s.uid_store(imap::uid_set(chunk), &q).await?.try_collect().await
                    })
                    .await?;
                }
                Ok(())
            }
            Cmd::MarkAllRead { folder_id } => {
                let (_, path, _) = self.db.folder(*folder_id)?;
                self.select(&path).await?;
                let s = self.ensure().await?;
                let uids: Vec<u32> = timed(CMD_TIMEOUT, "searching unread messages", s.uid_search("UNSEEN"))
                    .await?
                    .into_iter()
                    .collect();
                for chunk in uids.chunks(UID_CHUNK) {
                    let s = self.ensure().await?;
                    let _: Vec<Fetch> = timed(CMD_TIMEOUT, "marking messages read", async {
                        s.uid_store(imap::uid_set(chunk), "+FLAGS.SILENT (\\Seen)").await?.try_collect().await
                    })
                    .await?;
                }
                Ok(())
            }
            Cmd::Move { folder_id, uids, to } => {
                if uids.is_empty() {
                    return Ok(());
                }
                let (to_id, to_path, to_role) = match to {
                    Target::Role(role) => {
                        let (id, path) = self.target_folder(role).await?;
                        (id, path, Some(role.to_string()))
                    }
                    Target::Folder(id) => {
                        let (_, path, role) = self.db.folder(*id)?;
                        (*id, path, role)
                    }
                };
                let (_, path, _) = self.db.folder(*folder_id)?;
                if path == to_path {
                    return Ok(());
                }
                self.move_uids(&path, uids, &to_path).await?;
                self.sync_folder(to_id, &to_path, to_role.as_deref()).await?;
                Ok(())
            }
            Cmd::DeleteDraft { message_id } => {
                if message_id.trim().is_empty() {
                    return Ok(());
                }
                let Some((id, path)) = self.db.folder_by_role(self.account.id, "drafts")? else { return Ok(()) };
                self.select(&path).await?;
                let uids = self.find_message_id(message_id).await?;
                self.expunge_uids(&uids).await?;
                self.db.delete_by_message_id(id, message_id)?;
                self.changed();
                Ok(())
            }
            Cmd::Delete { folder_id, uids } => {
                if uids.is_empty() {
                    return Ok(());
                }
                let (_, path, _) = self.db.folder(*folder_id)?;
                self.select(&path).await?;
                self.expunge_uids(uids).await?;
                Ok(())
            }
            Cmd::EnsureSent { message_id, raw } => {
                let Some((sent_id, sent_path)) = self.db.folder_by_role(self.account.id, "sent")? else { return Ok(()) };
                self.sync_folder(sent_id, &sent_path, Some("sent")).await?;
                if !self.db.has_message_id(sent_id, message_id)? {
                    let s = self.ensure().await?;
                    timed(FETCH_TIMEOUT, "saving sent message", s.append(&sent_path, Some("(\\Seen)"), None, raw)).await?;
                    self.sync_folder(sent_id, &sent_path, Some("sent")).await?;
                }
                Ok(())
            }
            Cmd::SyncAll
            | Cmd::SyncFolder(_)
            | Cmd::FetchOlder { .. }
            | Cmd::SaveDraft { .. }
            | Cmd::Unsnooze { .. }
            | Cmd::Search { .. } => Ok(()),
        }
    }

    /// Moves uids out of `from` (MOVE, or COPY + delete where MOVE isn't supported).
    async fn move_uids(&mut self, from: &str, uids: &[u32], to: &str) -> Result<()> {
        if uids.is_empty() {
            return Ok(());
        }
        self.select(from).await?;
        for chunk in uids.chunks(UID_CHUNK) {
            let set = imap::uid_set(chunk);
            let s = self.ensure().await?;
            if timed(CMD_TIMEOUT, "moving messages", s.uid_mv(&set, to)).await.is_err() {
                let s = self.ensure().await?;
                timed(CMD_TIMEOUT, "copying messages", s.uid_copy(&set, to)).await?;
                self.expunge_uids(chunk).await?;
            }
        }
        Ok(())
    }

    /// Deletes uids in the selected folder. Uses UID EXPUNGE so nothing else marked deleted goes too.
    async fn expunge_uids(&mut self, uids: &[u32]) -> Result<()> {
        if uids.is_empty() {
            return Ok(());
        }
        for chunk in uids.chunks(UID_CHUNK) {
            let set = imap::uid_set(chunk);
            let s = self.ensure().await?;
            let _: Vec<Fetch> = timed(CMD_TIMEOUT, "marking messages deleted", async {
                s.uid_store(&set, "+FLAGS.SILENT (\\Deleted)").await?.try_collect().await
            })
            .await?;
            let uid_expunged = match tokio::time::timeout(CMD_TIMEOUT, async {
                match s.uid_expunge(&set).await {
                    Ok(stream) => stream.try_collect::<Vec<u32>>().await.is_ok(),
                    Err(_) => false,
                }
            })
            .await
            {
                Ok(ok) => ok,
                Err(_) => return Err(anyhow!("Timed out expunging messages")),
            };
            if !uid_expunged {
                let _: Vec<u32> = timed(CMD_TIMEOUT, "expunging messages", async {
                    s.expunge().await?.try_collect().await
                })
                .await?;
            }
        }
        Ok(())
    }

    /// Uids in the selected folder whose Message-ID header is `message_id`.
    async fn find_message_id(&mut self, message_id: &str) -> Result<Vec<u32>> {
        let mid = message_id.trim();
        if mid.is_empty() {
            return Ok(vec![]);
        }
        let s = self.ensure().await?;
        let q = if mid.is_ascii() {
            format!("HEADER Message-ID {}", imap::quote(mid))
        } else {
            format!("CHARSET UTF-8 HEADER Message-ID {}", imap::quote(mid))
        };
        Ok(timed(CMD_TIMEOUT, "searching by Message-ID", s.uid_search(q)).await?.into_iter().collect())
    }

    async fn save_draft(&mut self, message_id: &str, raw: &[u8]) -> Result<()> {
        let (id, path) = self.target_folder("drafts").await?;
        self.select(&path).await?;
        let old = self.find_message_id(message_id).await?;
        let s = self.ensure().await?;
        timed(FETCH_TIMEOUT, "saving draft", s.append(&path, Some("(\\Seen \\Draft)"), None, raw)).await?;
        self.expunge_uids(&old).await?;
        self.db.delete_by_message_id(id, message_id)?;
        self.sync_folder(id, &path, Some("drafts")).await?;
        Ok(())
    }

    async fn unsnooze(&mut self, message_id: &str, to_path: &str) -> Result<bool> {
        if message_id.trim().is_empty() {
            return Ok(false);
        }
        let Some((snoozed_id, snoozed_path)) = self.db.folder_by_role(self.account.id, "snoozed")? else { return Ok(false) };
        self.select(&snoozed_path).await?;
        let uids = self.find_message_id(message_id).await?;
        if uids.is_empty() {
            return Ok(false);
        }
        let (to_id, to_path, to_role) = match self.db.folder_by_path(self.account.id, to_path)? {
            Some((id, role)) if role.as_deref() != Some("snoozed") => (id, to_path.to_string(), role),
            _ => {
                let (id, path) = self.target_folder("inbox").await?;
                (id, path, Some("inbox".to_string()))
            }
        };
        self.move_uids(&snoozed_path, &uids, &to_path).await?;
        self.db.delete_by_message_id(snoozed_id, message_id)?;
        // Back as unread, like new mail.
        self.select(&to_path).await?;
        let back = self.find_message_id(message_id).await?;
        if !back.is_empty() {
            let s = self.ensure().await?;
            let _: Vec<Fetch> = timed(CMD_TIMEOUT, "marking unsnoozed message unread", async {
                s.uid_store(imap::uid_set(&back), "-FLAGS.SILENT (\\Seen)").await?.try_collect().await
            })
            .await?;
        }
        self.sync_folder(to_id, &to_path, to_role.as_deref()).await?;
        self.changed();
        Ok(true)
    }

    /// Searches `only`, or else every folder that could hold the mail (just All Mail where
    /// there is one), newest hits first, storing the ones not cached yet.
    async fn search(&mut self, criteria: &str, only: Option<&[i64]>) -> Result<Vec<i64>> {
        let folders = self.db.account_folders(self.account.id)?;
        let all_mail: Vec<_> = folders.iter().filter(|f| f.2.as_deref() == Some("all")).cloned().collect();
        let targets: Vec<_> = if let Some(only) = only {
            folders.into_iter().filter(|f| only.contains(&f.0)).collect()
        } else if all_mail.is_empty() {
            folders.into_iter().filter(|f| !matches!(f.2.as_deref(), Some("trash") | Some("junk"))).collect()
        } else {
            all_mail
        };
        let mut ids = Vec::new();
        for (folder_id, path, _) in targets {
            self.select(&path).await?;
            let s = self.ensure().await?;
            let mut uids: Vec<u32> = timed(CMD_TIMEOUT, "searching server", s.uid_search(criteria)).await?.into_iter().collect();
            uids.sort_unstable_by(|a, b| b.cmp(a));
            uids.truncate(SEARCH_HITS);
            let missing = self.db.missing_uids(folder_id, &uids)?;
            if !missing.is_empty() {
                let s = self.ensure().await?;
                let fetches: Vec<Fetch> = timed(FETCH_TIMEOUT, "fetching search results", async {
                    s.uid_fetch(imap::uid_set(&missing), HEADER_ITEMS).await?.try_collect().await
                })
                .await?;
                let found = headers(&fetches, |u| missing.contains(&u));
                self.db.insert_messages_as(self.account.id, folder_id, &found, true)?;
            }
            ids.extend(self.db.ids_for_uids(folder_id, &uids)?);
        }
        Ok(ids)
    }

    async fn fetch_older(&mut self, folder_id: i64) -> Result<usize> {
        let (_, path, _) = self.db.folder(folder_id)?;
        let (min_uid, _) = self.db.uid_range(folder_id)?;
        if min_uid <= 1 {
            return Ok(0);
        }
        self.select(&path).await?;
        let s = self.ensure().await?;
        let found = timed(CMD_TIMEOUT, "finding older messages", s.uid_search(format!("UID 1:{}", min_uid - 1))).await?;
        let mut uids: Vec<u32> = found.into_iter().filter(|u| *u < min_uid).collect();
        uids.sort_unstable();
        let batch: Vec<u32> = uids.iter().rev().take(BACKFILL).copied().collect();
        if batch.is_empty() {
            return Ok(0);
        }
        let mut fetched_total = 0;
        for chunk in batch.chunks(UID_CHUNK) {
            let s = self.ensure().await?;
            let fetches: Vec<Fetch> = timed(FETCH_TIMEOUT, "loading older headers", async {
                s.uid_fetch(imap::uid_set(chunk), HEADER_ITEMS).await?.try_collect().await
            })
            .await?;
            imap::confirm_alive(s).await?;
            let older = headers(&fetches, |uid| uid < min_uid);
            self.db.insert_messages(self.account.id, folder_id, &older)?;
            fetched_total += older.len();
        }
        self.changed();
        Ok(fetched_total)
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
            "drafts" => "Drafts",
            "snoozed" => "Snoozed",
            "inbox" => "INBOX",
            other => return Err(anyhow!("No {other} folder")),
        };
        let s = self.ensure().await?;
        timed(CMD_TIMEOUT, "creating folder", s.create(name)).await?;
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
        let mb = imap::select(s, path).await?;
        self.selected = Some(path.to_string());
        let uv = mb.uid_validity.unwrap_or(0);
        let mut changed = false;
        if self.db.folder_uidvalidity(folder_id)? != uv {
            self.db.reset_folder(folder_id, uv)?;
            changed = true;
        }
        let was_synced = self.db.folder_synced(folder_id)?;
        if mb.exists == 0 {
            changed |= self.db.reconcile_flags(folder_id, 0, &[])?;
            self.db.mark_folder_synced(folder_id)?;
            if changed {
                self.changed();
            }
            return Ok(());
        }

        let (_, max_uid) = self.db.uid_range(folder_id)?;
        let fetches: Vec<Fetch> = if max_uid == 0 {
            let start = mb.exists.saturating_sub(INITIAL_HEADERS - 1).max(1);
            let s = self.session.as_mut().unwrap();
            let list = timed(FETCH_TIMEOUT, "fetching headers", async {
                s.fetch(format!("{start}:*"), HEADER_ITEMS).await?.try_collect().await
            })
            .await?;
            imap::confirm_alive(s).await?;
            list
        } else if let Some(next_uid) = max_uid.checked_add(1) {
            let s = self.session.as_mut().unwrap();
            let list = timed(FETCH_TIMEOUT, "fetching new headers", async {
                s.uid_fetch(format!("{next_uid}:*"), HEADER_ITEMS).await?.try_collect().await
            })
            .await?;
            imap::confirm_alive(s).await?;
            list
        } else {
            vec![]
        };
        let new = headers(&fetches, |uid| uid > max_uid);

        if !new.is_empty() {
            let added = self.db.insert_messages(self.account.id, folder_id, &new)?;
            changed = true;
            // New Inbox mail goes through the rules, which then announce what's still there.
            // A first sync (`!was_synced`) is old mail: no rules, no notification.
            // Using `folders.synced` instead of `max_uid > 0` ensures an emptied Inbox (Inbox Zero)
            // still runs rules and notifies when new mail arrives.
            if role == Some("inbox") && was_synced && !added.is_empty() {
                self.db.queue_for_rules(self.account.id, &added, chrono::Utc::now().timestamp())?;
                self.rules_wake.notify_one();
            }
        }

        // Reconcile flags and deletions for everything we hold locally.
        let (min_uid, _) = self.db.uid_range(folder_id)?;
        if min_uid > 0 {
            let s = self.session.as_mut().unwrap();
            // The folder has mail, so this answer always holds at least its newest message.
            let flags = imap::fetch_flags(s, format!("{min_uid}:*"), true).await?;
            changed |= self.db.reconcile_flags(folder_id, min_uid, &flags)?;
            // Heal any server UIDs in the synced window that are missing locally (e.g. after a
            // failed optimistic move while offline) so messages never vanish permanently.
            let server_uids: Vec<u32> = flags.iter().map(|(u, _, _)| *u).collect();
            let mut missing = self.db.missing_uids(folder_id, &server_uids)?;
            missing.sort_unstable_by(|a, b| b.cmp(a));
            missing.truncate(BACKFILL);
            for chunk in missing.chunks(UID_CHUNK) {
                let s = self.session.as_mut().unwrap();
                let list: Vec<Fetch> = timed(FETCH_TIMEOUT, "restoring missing headers", async {
                    s.uid_fetch(imap::uid_set(chunk), HEADER_ITEMS).await?.try_collect().await
                })
                .await?;
                imap::confirm_alive(s).await?;
                let restored = headers(&list, |u| chunk.contains(&u));
                if !self.db.insert_messages(self.account.id, folder_id, &restored)?.is_empty() {
                    changed = true;
                }
            }
        }
        // Search hits below the window: check just those.
        let sparse = self.db.sparse_uids_below(folder_id, if min_uid > 0 { min_uid } else { u32::MAX })?;
        for chunk in sparse.chunks(UID_CHUNK) {
            let s = self.session.as_mut().unwrap();
            let flags = imap::fetch_flags(s, imap::uid_set(chunk), false).await?;
            changed |= self.db.reconcile_uids(folder_id, chunk, &flags)?;
        }
        self.db.mark_folder_synced(folder_id)?;
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
            let fetches: Vec<Fetch> = timed(FETCH_TIMEOUT, "prefetching bodies", async {
                s.uid_fetch(imap::uid_set(chunk), "(UID BODY.PEEK[])").await?.try_collect().await
            })
            .await?;
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
        let started = std::time::Instant::now();
        match idle_once(&account, &db, &tx).await {
            Ok(()) => return, // worker gone
            Err(_) => {
                // Reset backoff after a healthy session so routine server disconnects don't
                // leave push notifications permanently stuck at a 5-minute delay.
                if started.elapsed() >= Duration::from_secs(30) {
                    backoff = 5;
                }
                tokio::time::sleep(Duration::from_secs(backoff)).await;
                backoff = (backoff * 2).min(300);
            }
        }
    }
}

async fn idle_once(account: &Account, db: &Db, tx: &mpsc::UnboundedSender<Cmd>) -> Result<()> {
    let pw = secrets::get(&account.email)?;
    let mut session = imap::connect(&account.imap_host, account.imap_port, &account.email, &pw).await?;
    timed(CMD_TIMEOUT, "selecting INBOX for IDLE", session.select("INBOX")).await?;
    loop {
        let mut idle = session.idle();
        timed(CMD_TIMEOUT, "starting IDLE", idle.init()).await?;
        let res = {
            let (wait, _stop) = idle.wait_with_timeout(Duration::from_secs(20 * 60));
            wait.await?
        };
        session = timed(CMD_TIMEOUT, "ending IDLE", idle.done()).await?;
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
