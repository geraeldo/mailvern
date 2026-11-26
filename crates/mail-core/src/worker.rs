//! One worker per account. It owns the IMAP connection and processes commands in order.
//! A second connection sits in read-only IDLE on the inbox and pokes the worker when mail arrives.

use crate::db::{Db, FolderSyncState, NewMessage, PendingOp, PendingOpPayload};
use crate::imap::{self, CopyUid, FetchedHeader, FolderStatus, ServerCaps, Session, CMD_TIMEOUT, FETCH_TIMEOUT};
use crate::model::{Account, Event};
use crate::{parse, secrets};
use anyhow::{anyhow, Result};
use async_imap::extensions::idle::IdleResponse;
use futures::TryStreamExt;
use imap_proto::types::{MailboxDatum, Response};
use std::collections::{HashSet, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc, oneshot, Notify};

const INITIAL_HEADERS: u32 = 500;
const POLL_EVERY: Duration = Duration::from_secs(180);
/// How many older headers to fetch each time the list reaches the end of the local cache.
const BACKFILL: usize = 500;
/// Most hits fetched per folder by a server search.
const SEARCH_HITS: usize = 50;
/// Maximum UIDs sent in a single IMAP command so fragmented sets stay well under line limits.
const UID_CHUNK: usize = 200;
/// Prefetch chunk limits: cap both message count and cumulative size to avoid timeout loops.
const PREFETCH_MAX_COUNT: usize = 15;
const PREFETCH_MAX_BYTES: u32 = 4_000_000;
/// Maximum size of any single email that will be predownloaded in the background (4 MiB).
const PREFETCH_MAX_EMAIL_BYTES: u32 = 4 * 1024 * 1024;
/// Maximum cached compressed raw bodies per account before evicting oldest non-Inbox bodies
/// (~150 MiB compressed holds ~450–600 MiB of raw RFC 822 mail).
const MAX_BODY_CACHE_BYTES: i64 = 150 * 1024 * 1024;
/// Liveness check threshold before reusing an idle worker IMAP session.
const STALE_SESSION_AFTER: Duration = Duration::from_secs(90);
/// IDLE refresh interval (short enough to recover quickly after sleep/NAT drops).
const IDLE_REFRESH: Duration = Duration::from_secs(9 * 60);
/// Suppresses self-triggered IDLE FETCH/EXPUNGE echoes right after our own Inbox writes.
const ECHO_SUPPRESS_MS: i64 = 2_500;
/// Maximum retry attempts for a durable pending_op before giving up.
const MAX_OP_ATTEMPTS: i64 = 12;

fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

async fn timed<T, E: Into<anyhow::Error>>(
    dur: Duration,
    label: &str,
    fut: impl Future<Output = std::result::Result<T, E>>,
) -> Result<T> {
    tokio::time::timeout(dur, fut)
        .await
        .map_err(|_| anyhow!("Timed out {label}"))?
        .map_err(Into::into)
}

type BoxFut<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

pub enum Cmd {
    SyncAll { force: bool },
    SyncFolder { folder_id: i64, from_idle: bool },
    /// Replay persisted mutations from `pending_ops` immediately.
    FlushOps,
    FetchBody { folder_id: i64, uid: u32, reply: oneshot::Sender<Result<()>> },
    /// Download headers older than anything held locally. Replies with how many arrived (0 = none left).
    FetchOlder { folder_id: i64, reply: oneshot::Sender<Result<usize>> },
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
    CreateFolder { name: String, parent_id: Option<i64>, reply: oneshot::Sender<Result<crate::model::Folder>> },
    RenameFolder { folder_id: i64, new_name: String, reply: oneshot::Sender<Result<()>> },
    DeleteFolder { folder_id: i64, reply: oneshot::Sender<Result<()>> },
}

pub struct Worker {
    account: Account,
    db: Arc<Db>,
    events: broadcast::Sender<Event>,
    /// Wakes the rule runner after new Inbox mail is queued for it.
    rules_wake: Arc<Notify>,
    /// Shared timestamp (epoch ms) of our last Inbox mutation, so IDLE ignores our own echoes.
    last_inbox_write_ms: Arc<AtomicI64>,
    rx: mpsc::UnboundedReceiver<Cmd>,
    session: Option<Session>,
    caps: ServerCaps,
    last_activity: Option<Instant>,
    cached_password: Option<String>,
    selected: Option<String>,
    /// Urgent non-sync commands (always run ahead of background syncs).
    urgent: VecDeque<Cmd>,
    /// Coalesced SyncAll request: `Some(force)`.
    pending_sync_all: Option<bool>,
    /// Coalesced per-folder sync requests: `(folder_id, from_idle)`.
    pending_sync_folders: VecDeque<(i64, bool)>,
    /// Avoids queueing multiple redundant `FlushOps` markers.
    flush_queued: bool,
}

impl Worker {
    pub fn new(
        account: Account,
        db: Arc<Db>,
        events: broadcast::Sender<Event>,
        rules_wake: Arc<Notify>,
        last_inbox_write_ms: Arc<AtomicI64>,
        rx: mpsc::UnboundedReceiver<Cmd>,
    ) -> Self {
        Worker {
            account,
            db,
            events,
            rules_wake,
            last_inbox_write_ms,
            rx,
            session: None,
            caps: ServerCaps::default(),
            last_activity: None,
            cached_password: None,
            selected: None,
            urgent: VecDeque::new(),
            pending_sync_all: None,
            pending_sync_folders: VecDeque::new(),
            flush_queued: false,
        }
    }

    fn emit(&self, e: Event) {
        let _ = self.events.send(e);
    }

    fn status(&self, state: &str, message: Option<String>) {
        self.emit(Event::Status { account_id: self.account.id, state: state.into(), message });
    }

    fn changed(&self, folder_id: Option<i64>) {
        self.emit(Event::Changed { account_id: self.account.id, folder_id });
    }

    fn note_inbox_write(&self) {
        self.last_inbox_write_ms.store(now_ms(), Ordering::Relaxed);
    }

    fn enqueue(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::SyncAll { force } => {
                let merged = self.pending_sync_all.unwrap_or(false) || force;
                self.pending_sync_all = Some(merged);
                self.pending_sync_folders.clear();
            }
            Cmd::SyncFolder { folder_id, from_idle } => {
                if self.pending_sync_all.is_some() {
                    return;
                }
                if let Some(existing) = self.pending_sync_folders.iter_mut().find(|(id, _)| *id == folder_id) {
                    // A manual folder check overrides an IDLE-triggered light check.
                    existing.1 &= from_idle;
                } else {
                    self.pending_sync_folders.push_back((folder_id, from_idle));
                }
            }
            Cmd::FlushOps => {
                if !self.flush_queued {
                    self.flush_queued = true;
                    self.urgent.push_back(Cmd::FlushOps);
                }
            }
            other => self.urgent.push_back(other),
        }
    }

    /// Drains all currently queued commands from `rx` and coalesces duplicate sync requests.
    fn absorb_rx(&mut self) {
        while let Ok(cmd) = self.rx.try_recv() {
            self.enqueue(cmd);
        }
    }

    fn next_pending(&mut self) -> Option<Cmd> {
        self.absorb_rx();
        if let Some(cmd) = self.urgent.pop_front() {
            if matches!(cmd, Cmd::FlushOps) {
                self.flush_queued = false;
            }
            return Some(cmd);
        }
        if let Some((folder_id, from_idle)) = self.pending_sync_folders.pop_front() {
            return Some(Cmd::SyncFolder { folder_id, from_idle });
        }
        if let Some(force) = self.pending_sync_all.take() {
            return Some(Cmd::SyncAll { force });
        }
        None
    }

    pub async fn run(mut self) {
        // Start ticking AFTER the first POLL_EVERY interval: startup already queues an explicit SyncAll.
        let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + POLL_EVERY, POLL_EVERY);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            let cmd = if let Some(c) = self.next_pending() {
                c
            } else {
                tokio::select! {
                    c = self.rx.recv() => match c {
                        Some(c) => {
                            self.enqueue(c);
                            continue;
                        }
                        None => break,
                    },
                    _ = ticker.tick() => Cmd::SyncAll { force: false },
                }
            };
            self.handle(cmd).await;
        }
        if let Some(mut s) = self.session.take() {
            let _ = s.logout().await;
        }
    }

    async fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::SyncAll { force } => {
                let had_session = self.session.is_some();
                let mut res = self.sync_all(force).await;
                if res.is_err() && had_session {
                    self.reset();
                    res = self.sync_all(force).await;
                }
                if let Err(e) = res {
                    self.fail(e);
                }
            }
            Cmd::SyncFolder { folder_id, from_idle } => {
                let had_session = self.session.is_some();
                let mut res = self.run_sync_folder_cmd(folder_id, from_idle).await;
                if res.is_err() && had_session {
                    self.reset();
                    res = self.run_sync_folder_cmd(folder_id, from_idle).await;
                }
                if let Err(e) = res {
                    self.fail(e);
                }
            }
            other => self.handle_quick(other).await,
        }
    }

    async fn run_sync_folder_cmd(&mut self, folder_id: i64, from_idle: bool) -> Result<()> {
        self.status("syncing", None);
        let (_, path, role) = self.db.folder(folder_id)?;
        let _ = self.flush_pending_ops(Some(folder_id)).await?;
        self.sync_folder(folder_id, &path, role.as_deref(), !from_idle).await?;
        // IDLE pokes only prefetch a small burst so new arrivals stay snappy without blocking on 300 bodies.
        let cap = if from_idle { Some(PREFETCH_MAX_COUNT) } else { None };
        self.prefetch(folder_id, &path, role.as_deref(), cap).await?;
        self.status("idle", None);
        Ok(())
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
    async fn handle_quick(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::FlushOps => {
                let had_session = self.session.is_some();
                let mut res = self.flush_pending_ops(None).await;
                if res.is_err() && had_session {
                    self.reset();
                    res = self.flush_pending_ops(None).await;
                }
                if let Err(e) = res {
                    self.fail(e);
                }
            }
            Cmd::FetchBody { folder_id, uid, reply } => {
                let res = self.fetch_body_resilient(folder_id, uid).await;
                let _ = reply.send(res);
            }
            Cmd::FetchOlder { folder_id, reply } => {
                let res = self.retry(true, |w| Box::pin(w.fetch_older(folder_id))).await;
                let _ = reply.send(res);
            }
            Cmd::SaveDraft { message_id, raw, reply } => {
                let res = self
                    .retry(true, |w| {
                        let (m, r) = (message_id.clone(), raw.clone());
                        Box::pin(async move { w.save_draft(&m, &r).await })
                    })
                    .await;
                let _ = reply.send(res);
            }
            Cmd::Unsnooze { message_id, to_path, reply } => {
                let res = self
                    .retry(true, |w| {
                        let (m, p) = (message_id.clone(), to_path.clone());
                        Box::pin(async move { w.unsnooze(&m, &p).await })
                    })
                    .await;
                let _ = reply.send(res);
            }
            Cmd::Search { criteria, folders, reply } => {
                let res = self
                    .retry(false, |w| {
                        let (t, f) = (criteria.clone(), folders.clone());
                        Box::pin(async move { w.search(&t, f.as_deref()).await })
                    })
                    .await;
                let _ = reply.send(res);
            }
            Cmd::DeleteDraft { message_id } => {
                let _ = self
                    .retry(true, |w| {
                        let m = message_id.clone();
                        Box::pin(async move { w.delete_draft(&m).await })
                    })
                    .await;
            }
            Cmd::EnsureSent { message_id, raw } => {
                let _ = self
                    .retry(true, |w| {
                        let (m, r) = (message_id.clone(), raw.clone());
                        Box::pin(async move { w.ensure_sent(&m, &r).await })
                    })
                    .await;
            }
            Cmd::CreateFolder { name, parent_id, reply } => {
                let res = self
                    .retry(false, |w| {
                        let n = name.clone();
                        Box::pin(async move { w.create_folder(&n, parent_id).await })
                    })
                    .await;
                let _ = reply.send(res);
            }
            Cmd::RenameFolder { folder_id, new_name, reply } => {
                let res = self
                    .retry(false, |w| {
                        let n = new_name.clone();
                        Box::pin(async move { w.rename_folder(folder_id, &n).await })
                    })
                    .await;
                let _ = reply.send(res);
            }
            Cmd::DeleteFolder { folder_id, reply } => {
                let res = self
                    .retry(false, |w| Box::pin(async move { w.delete_folder(folder_id).await }))
                    .await;
                let _ = reply.send(res);
            }
            Cmd::SyncAll { .. } | Cmd::SyncFolder { .. } => {}
        }
    }

    /// Fetches a single message body. A genuinely missing message on a healthy connection
    /// returns an error to the caller WITHOUT tearing down the session or setting account error.
    async fn fetch_body_resilient(&mut self, folder_id: i64, uid: u32) -> Result<()> {
        let had_session = self.session.is_some();
        let mut res = self.fetch_body_once(folder_id, uid).await;
        if let Err(ref e) = res {
            if !is_not_found_err(e) && had_session {
                self.reset();
                res = self.fetch_body_once(folder_id, uid).await;
            }
        }
        match res {
            Ok(()) => Ok(()),
            Err(e) if is_not_found_err(&e) => Err(e),
            Err(e) => {
                let msg = format!("{e:#}");
                self.fail(e);
                Err(anyhow!(msg))
            }
        }
    }

    async fn fetch_body_once(&mut self, folder_id: i64, uid: u32) -> Result<()> {
        let (_, path, _) = self.db.folder(folder_id)?;
        self.select(&path).await?;
        let s = self.ensure().await?;
        let fetched = imap::fetch_bodies(s, &uid.to_string()).await?;
        self.touch();
        let (_, raw) = fetched
            .into_iter()
            .find(|(u, _)| *u == uid)
            .ok_or_else(|| anyhow!("Message no longer on server"))?;
        let (raw, info) = tokio::task::spawn_blocking(move || {
            let info = parse::body_info(&raw);
            (raw, info)
        })
        .await
        .map_err(|_| anyhow!("Parsing message failed"))?;
        self.db.store_body(folder_id, uid, &raw, &info)?;
        Ok(())
    }

    fn reset(&mut self) {
        self.session = None;
        self.selected = None;
        self.last_activity = None;
    }

    fn touch(&mut self) {
        self.last_activity = Some(Instant::now());
    }

    fn fail(&mut self, e: anyhow::Error) {
        self.reset();
        self.status("error", Some(format!("{e:#}")));
    }

    fn password(&mut self) -> Result<String> {
        if let Some(pw) = &self.cached_password {
            return Ok(pw.clone());
        }
        let pw = secrets::get(&self.account.email)?;
        self.cached_password = Some(pw.clone());
        Ok(pw)
    }

    async fn ensure(&mut self) -> Result<&mut Session> {
        if let Some(last) = self.last_activity {
            if self.session.is_some() && last.elapsed() >= STALE_SESSION_AFTER {
                let alive = {
                    let s = self.session.as_mut().unwrap();
                    imap::ping(s).await.is_ok()
                };
                if alive {
                    self.touch();
                } else {
                    self.reset();
                }
            }
        }
        if self.session.is_none() {
            let pw = self.password()?;
            let mut s = match imap::connect(&self.account.imap_host, self.account.imap_port, &self.account.email, &pw).await {
                Ok(s) => s,
                Err(e) => {
                    if format!("{e:#}").contains("Sign-in failed") {
                        self.cached_password = None;
                    }
                    return Err(e);
                }
            };
            self.caps = imap::capabilities(&mut s).await.unwrap_or_default();
            self.session = Some(s);
            self.selected = None;
            self.touch();
        }
        Ok(self.session.as_mut().unwrap())
    }

    async fn select(&mut self, path: &str) -> Result<()> {
        if self.selected.as_deref() == Some(path) {
            return Ok(());
        }
        let condstore = self.caps.condstore;
        let s = self.ensure().await?;
        imap::select_folder(s, path, condstore).await?;
        self.touch();
        self.selected = Some(path.to_string());
        Ok(())
    }

    /// Replays durable `pending_ops` from SQLite onto the server.
    /// Returns true if any ops were processed.
    async fn flush_pending_ops(&mut self, only_folder: Option<i64>) -> Result<bool> {
        let ops = self.db.due_pending_ops(self.account.id, only_folder, now_secs())?;
        if ops.is_empty() {
            return Ok(false);
        }
        let mut ran = false;
        for op in ops {
            match self.exec_pending_op(&op).await {
                Ok(()) => {
                    self.db.delete_pending_op(op.id)?;
                    ran = true;
                }
                Err(e) => {
                    // Drop permanently unrecoverable ops (e.g. deleted local folder) or after max retries.
                    let permanent = is_permanent_op_err(&e) || op.attempts + 1 >= MAX_OP_ATTEMPTS;
                    if permanent {
                        eprintln!("pending_op {} dropped: {e:#}", op.id);
                        self.db.delete_pending_op(op.id)?;
                    } else {
                        let delay = 30 * (1i64 << op.attempts.min(6));
                        self.db.fail_pending_op(op.id, now_secs().saturating_add(delay))?;
                        return Err(e);
                    }
                }
            }
        }
        Ok(ran)
    }

    async fn exec_pending_op(&mut self, op: &PendingOp) -> Result<()> {
        let (_, path, src_role) = self.db.folder(op.folder_id)?;
        let is_inbox_src = src_role.as_deref() == Some("inbox");
        match &op.payload {
            PendingOpPayload::Store { flag, add } => {
                if op.uids.is_empty() {
                    return Ok(());
                }
                if is_inbox_src {
                    self.note_inbox_write();
                }
                self.select(&path).await?;
                let q = format!("{}FLAGS.SILENT ({})", if *add { "+" } else { "-" }, flag);
                for chunk in op.uids.chunks(UID_CHUNK) {
                    let s = self.ensure().await?;
                    let _: Vec<async_imap::types::Fetch> = timed(CMD_TIMEOUT, "updating flags", async {
                        s.uid_store(imap::uid_set(chunk), &q).await?.try_collect().await
                    })
                    .await?;
                    self.touch();
                }
                if is_inbox_src {
                    self.note_inbox_write();
                }
                self.db.invalidate_folder_counts(op.folder_id)?;
                Ok(())
            }
            PendingOpPayload::MarkAllRead => {
                if is_inbox_src {
                    self.note_inbox_write();
                }
                self.select(&path).await?;
                let s = self.ensure().await?;
                let uids = imap::uid_search(s, "UNSEEN").await?;
                self.touch();
                for chunk in uids.chunks(UID_CHUNK) {
                    let s = self.ensure().await?;
                    let _: Vec<async_imap::types::Fetch> = timed(CMD_TIMEOUT, "marking messages read", async {
                        s.uid_store(imap::uid_set(chunk), "+FLAGS.SILENT (\\Seen)").await?.try_collect().await
                    })
                    .await?;
                    self.touch();
                }
                if is_inbox_src {
                    self.note_inbox_write();
                }
                self.db.invalidate_folder_counts(op.folder_id)?;
                Ok(())
            }
            PendingOpPayload::Delete => {
                if op.uids.is_empty() {
                    return Ok(());
                }
                if is_inbox_src {
                    self.note_inbox_write();
                }
                self.select(&path).await?;
                self.expunge_uids(&op.uids).await?;
                if is_inbox_src {
                    self.note_inbox_write();
                }
                self.db.invalidate_folder_counts(op.folder_id)?;
                Ok(())
            }
            PendingOpPayload::EmptyFolder => {
                self.select(&path).await?;
                let s = self.ensure().await?;
                let uids = imap::uid_search(s, "ALL").await?;
                self.touch();
                if !uids.is_empty() {
                    self.expunge_uids(&uids).await?;
                }
                self.db.invalidate_folder_counts(op.folder_id)?;
                Ok(())
            }
            PendingOpPayload::MoveRole { role } => {
                if op.uids.is_empty() {
                    return Ok(());
                }
                let (to_id, to_path) = self.target_folder(role).await?;
                if path == to_path {
                    return Ok(());
                }
                self.exec_move((op.folder_id, &path, is_inbox_src), &op.uids, (to_id, &to_path, Some(role.as_str()))).await
            }
            PendingOpPayload::MoveFolder { to_folder_id } => {
                if op.uids.is_empty() {
                    return Ok(());
                }
                let (_, to_path, to_role) = self.db.folder(*to_folder_id)?;
                if path == to_path {
                    return Ok(());
                }
                self.exec_move((op.folder_id, &path, is_inbox_src), &op.uids, (*to_folder_id, &to_path, to_role.as_deref())).await
            }
        }
    }

    fn infer_delimiter(&self) -> char {
        if let Ok(folders) = self.db.account_folders(self.account.id) {
            for (_, path, _) in &folders {
                if path.contains('/') {
                    return '/';
                }
                if path.contains('.') {
                    return '.';
                }
            }
        }
        '/'
    }

    async fn create_folder(&mut self, name: &str, parent_id: Option<i64>) -> Result<crate::model::Folder> {
        let clean = name.trim();
        if clean.is_empty() {
            return Err(anyhow!("Folder name can't be empty"));
        }
        if clean.contains(['\r', '\n', '\0']) {
            return Err(anyhow!("Folder name contains invalid characters"));
        }
        let delim = self.infer_delimiter();
        let path = match parent_id {
            Some(pid) => {
                let (acc, parent_path, _) = self.db.folder(pid)?;
                if acc != self.account.id {
                    return Err(anyhow!("Parent folder belongs to another account"));
                }
                format!("{parent_path}{delim}{clean}")
            }
            None => clean.to_string(),
        };
        let s = self.ensure().await?;
        imap::create_folder(s, &path).await?;
        self.touch();
        let id = self.db.upsert_folder(self.account.id, &path, clean, None)?;
        let _ = self.events.send(Event::Changed { account_id: self.account.id, folder_id: None });
        Ok(crate::model::Folder {
            id,
            account_id: self.account.id,
            path,
            name: clean.to_string(),
            role: None,
            unread: 0,
            position: None,
        })
    }

    async fn rename_folder(&mut self, folder_id: i64, new_name: &str) -> Result<()> {
        let clean = new_name.trim();
        if clean.is_empty() {
            return Err(anyhow!("Folder name can't be empty"));
        }
        if clean.contains(['\r', '\n', '\0']) {
            return Err(anyhow!("Folder name contains invalid characters"));
        }
        let (acc, old_path, role) = self.db.folder(folder_id)?;
        if acc != self.account.id {
            return Err(anyhow!("Folder belongs to another account"));
        }
        if role.is_some() {
            return Err(anyhow!("Standard folders can't be renamed"));
        }
        let new_path = if let Some((prefix, _)) = old_path.rsplit_once('/') {
            format!("{prefix}/{clean}")
        } else if let Some((prefix, _)) = old_path.rsplit_once('.') {
            format!("{prefix}.{clean}")
        } else {
            clean.to_string()
        };
        if new_path == old_path {
            return Ok(());
        }
        if self.selected.as_deref() == Some(old_path.as_str()) {
            self.selected = None;
        }
        let s = self.ensure().await?;
        imap::rename_folder(s, &old_path, &new_path).await?;
        self.touch();
        self.db.rename_folder_local(folder_id, &new_path, clean)?;
        let _ = self.events.send(Event::Changed { account_id: self.account.id, folder_id: None });
        Ok(())
    }

    async fn delete_folder(&mut self, folder_id: i64) -> Result<()> {
        let (acc, path, role) = self.db.folder(folder_id)?;
        if acc != self.account.id {
            return Err(anyhow!("Folder belongs to another account"));
        }
        if role.is_some() {
            return Err(anyhow!("Standard folders can't be deleted"));
        }
        if self.selected.as_deref() == Some(path.as_str()) {
            let _ = self.select("INBOX").await;
        }
        let s = self.ensure().await?;
        imap::delete_folder(s, &path).await?;
        self.touch();
        self.db.delete_folder_local(folder_id)?;
        let _ = self.events.send(Event::Changed { account_id: self.account.id, folder_id: None });
        Ok(())
    }

    async fn exec_move(
        &mut self,
        from: (i64, &str, bool),
        uids: &[u32],
        to: (i64, &str, Option<&str>),
    ) -> Result<()> {
        let (from_id, from_path, is_inbox_src) = from;
        let (to_id, to_path, to_role) = to;
        let is_inbox_dst = to_role == Some("inbox");
        if is_inbox_src || is_inbox_dst {
            self.note_inbox_write();
        }
        let copyuids = self.move_uids(from_path, uids, to_path).await?;
        if is_inbox_src || is_inbox_dst {
            self.note_inbox_write();
        }
        self.db.invalidate_folder_counts(from_id)?;
        self.db.invalidate_folder_counts(to_id)?;

        // Fast path: if the server returned COPYUID and destination is synced with matching
        // UIDVALIDITY, fetch ONLY the newly created destination UIDs (not the whole folder!)
        // and hydrate their bodies locally from `moved_bodies`.
        let dst_state = self.db.folder_sync_state(to_id)?;
        let dst_uids: Vec<u32> = copyuids
            .iter()
            .filter(|cu| dst_state.synced && cu.uid_validity == dst_state.uidvalidity)
            .flat_map(|cu| cu.pairs.iter().map(|&(_, d)| d))
            .collect();

        if !dst_uids.is_empty() {
            self.select(to_path).await?;
            for chunk in dst_uids.chunks(UID_CHUNK) {
                let s = self.ensure().await?;
                let hdrs = imap::fetch_headers(s, &imap::uid_set(chunk), true).await?;
                self.touch();
                let msgs = to_new_messages(&hdrs, |u| chunk.contains(&u));
                self.db.insert_messages(self.account.id, to_id, &msgs)?;
            }
            let _ = self.db.copy_existing_bodies_for_folder(self.account.id, to_id)?;
            self.changed(Some(to_id));
        } else if to_role != Some("all") {
            // Without COPYUID, only fetch newly arrived headers in the destination (skipping
            // expensive full All Mail flag scans on every single archive click).
            self.sync_folder_new_only(to_id, to_path).await?;
        }
        Ok(())
    }

    /// Lightweight destination refresh after a Move when COPYUID wasn't provided: fetches only
    /// messages with UID > max_uid and hydrates their bodies from stash/other folders.
    async fn sync_folder_new_only(&mut self, folder_id: i64, path: &str) -> Result<()> {
        let state = self.db.folder_sync_state(folder_id)?;
        if !state.synced {
            return Ok(());
        }
        let (_, max_uid) = self.db.uid_range(folder_id)?;
        if max_uid == 0 {
            return Ok(());
        }
        let condstore = self.caps.condstore;
        let s = self.ensure().await?;
        let mb = imap::select_folder(s, path, condstore).await?;
        self.touch();
        self.selected = Some(path.to_string());
        if mb.uid_validity != state.uidvalidity || mb.exists == 0 {
            return Ok(());
        }
        if mb.uid_next.is_some_and(|next| next <= max_uid.saturating_add(1)) {
            return Ok(());
        }
        if let Some(next_uid) = max_uid.checked_add(1) {
            let s = self.session.as_mut().unwrap();
            let hdrs = imap::fetch_headers(s, &format!("{next_uid}:*"), true).await?;
            self.touch();
            let new = to_new_messages(&hdrs, |u| u > max_uid);
            if !new.is_empty() {
                self.db.insert_messages(self.account.id, folder_id, &new)?;
                let _ = self.db.copy_existing_bodies_for_folder(self.account.id, folder_id)?;
                self.changed(Some(folder_id));
            }
        }
        Ok(())
    }

    /// Moves uids out of `from` (UID MOVE when supported, else UID COPY + expunge).
    /// Returns any `COPYUID` mappings reported by the server.
    async fn move_uids(&mut self, from: &str, uids: &[u32], to: &str) -> Result<Vec<CopyUid>> {
        if uids.is_empty() {
            return Ok(vec![]);
        }
        self.select(from).await?;
        let mut copyuids = Vec::new();
        for chunk in uids.chunks(UID_CHUNK) {
            let set = imap::uid_set(chunk);
            let moved = if self.caps.move_ext {
                let s = self.ensure().await?;
                match imap::uid_move_or_copy(s, &set, to, true).await {
                    Ok(cu) => {
                        self.touch();
                        if let Some(c) = cu {
                            copyuids.push(c);
                        }
                        true
                    }
                    Err(_) => false,
                }
            } else {
                false
            };
            if !moved {
                let s = self.ensure().await?;
                let cu = imap::uid_move_or_copy(s, &set, to, false).await?;
                self.touch();
                if let Some(c) = cu {
                    copyuids.push(c);
                }
                self.expunge_uids(chunk).await?;
            }
        }
        Ok(copyuids)
    }

    /// Deletes uids in the selected folder. Uses UID EXPUNGE when available so nothing else marked deleted goes too.
    async fn expunge_uids(&mut self, uids: &[u32]) -> Result<()> {
        if uids.is_empty() {
            return Ok(());
        }
        for chunk in uids.chunks(UID_CHUNK) {
            let set = imap::uid_set(chunk);
            let uidplus = self.caps.uidplus;
            let s = self.ensure().await?;
            let _: Vec<async_imap::types::Fetch> = timed(CMD_TIMEOUT, "marking messages deleted", async {
                s.uid_store(&set, "+FLAGS.SILENT (\\Deleted)").await?.try_collect().await
            })
            .await?;
            let uid_expunged = if uidplus {
                match tokio::time::timeout(CMD_TIMEOUT, async {
                    match s.uid_expunge(&set).await {
                        Ok(stream) => stream.try_collect::<Vec<u32>>().await.is_ok(),
                        Err(_) => false,
                    }
                })
                .await
                {
                    Ok(ok) => ok,
                    Err(_) => return Err(anyhow!("Timed out expunging messages")),
                }
            } else {
                false
            };
            if !uid_expunged {
                let _: Vec<u32> = timed(CMD_TIMEOUT, "expunging messages", async {
                    s.expunge().await?.try_collect().await
                })
                .await?;
            }
            self.touch();
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
        let uids = imap::uid_search(s, &q).await?;
        self.touch();
        Ok(uids)
    }

    async fn delete_draft(&mut self, message_id: &str) -> Result<()> {
        if message_id.trim().is_empty() {
            return Ok(());
        }
        let Some((id, path)) = self.db.folder_by_role(self.account.id, "drafts")? else { return Ok(()) };
        self.select(&path).await?;
        let uids = self.find_message_id(message_id).await?;
        self.expunge_uids(&uids).await?;
        self.db.invalidate_folder_counts(id)?;
        self.db.delete_by_message_id(id, message_id)?;
        self.changed(Some(id));
        Ok(())
    }

    async fn ensure_sent(&mut self, message_id: &str, raw: &[u8]) -> Result<()> {
        let Some((sent_id, sent_path)) = self.db.folder_by_role(self.account.id, "sent")? else { return Ok(()) };
        self.sync_folder(sent_id, &sent_path, Some("sent"), false).await?;
        if !self.db.has_message_id(sent_id, message_id)? {
            let s = self.ensure().await?;
            timed(FETCH_TIMEOUT, "saving sent message", s.append(&sent_path, Some("(\\Seen)"), None, raw)).await?;
            self.touch();
            self.db.invalidate_folder_counts(sent_id)?;
            self.sync_folder(sent_id, &sent_path, Some("sent"), false).await?;
        }
        Ok(())
    }

    async fn save_draft(&mut self, message_id: &str, raw: &[u8]) -> Result<()> {
        let (id, path) = self.target_folder("drafts").await?;
        self.select(&path).await?;
        let old = self.find_message_id(message_id).await?;
        let s = self.ensure().await?;
        timed(FETCH_TIMEOUT, "saving draft", s.append(&path, Some("(\\Seen \\Draft)"), None, raw)).await?;
        self.touch();
        self.expunge_uids(&old).await?;
        self.db.invalidate_folder_counts(id)?;
        self.db.delete_by_message_id(id, message_id)?;
        self.sync_folder(id, &path, Some("drafts"), false).await?;
        Ok(())
    }

    async fn unsnooze(&mut self, message_id: &str, to_path: &str) -> Result<bool> {
        if message_id.trim().is_empty() {
            return Ok(false);
        }
        let Some((snoozed_id, snoozed_path)) = self.db.folder_by_role(self.account.id, "snoozed")? else {
            return Ok(false);
        };
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
        if to_role.as_deref() == Some("inbox") {
            self.note_inbox_write();
        }
        let _ = self.move_uids(&snoozed_path, &uids, &to_path).await?;
        self.db.invalidate_folder_counts(snoozed_id)?;
        self.db.invalidate_folder_counts(to_id)?;
        self.db.delete_by_message_id(snoozed_id, message_id)?;
        // Back as unread, like new mail.
        self.select(&to_path).await?;
        let back = self.find_message_id(message_id).await?;
        if !back.is_empty() {
            let s = self.ensure().await?;
            let _: Vec<async_imap::types::Fetch> = timed(CMD_TIMEOUT, "marking unsnoozed message unread", async {
                s.uid_store(imap::uid_set(&back), "-FLAGS.SILENT (\\Seen)").await?.try_collect().await
            })
            .await?;
            self.touch();
        }
        if to_role.as_deref() == Some("inbox") {
            self.note_inbox_write();
        }
        self.sync_folder(to_id, &to_path, to_role.as_deref(), false).await?;
        self.changed(Some(snoozed_id));
        self.changed(Some(to_id));
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
            let mut uids = imap::uid_search(s, criteria).await?;
            self.touch();
            uids.sort_unstable_by(|a, b| b.cmp(a));
            uids.truncate(SEARCH_HITS);
            let missing = self.db.missing_uids(folder_id, &uids)?;
            if !missing.is_empty() {
                for chunk in missing.chunks(UID_CHUNK) {
                    let s = self.ensure().await?;
                    let hdrs = imap::fetch_headers(s, &imap::uid_set(chunk), true).await?;
                    self.touch();
                    let found = to_new_messages(&hdrs, |u| chunk.contains(&u));
                    self.db.insert_messages_as(self.account.id, folder_id, &found, true)?;
                }
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
        let found = imap::uid_search(s, &format!("UID 1:{}", min_uid - 1)).await?;
        self.touch();
        let mut uids: Vec<u32> = found.into_iter().filter(|u| *u < min_uid).collect();
        uids.sort_unstable();
        let batch: Vec<u32> = uids.iter().rev().take(BACKFILL).copied().collect();
        if batch.is_empty() {
            return Ok(0);
        }
        let mut fetched_total = 0;
        for chunk in batch.chunks(UID_CHUNK) {
            let s = self.ensure().await?;
            let hdrs = imap::fetch_headers(s, &imap::uid_set(chunk), true).await?;
            self.touch();
            let older = to_new_messages(&hdrs, |uid| uid < min_uid);
            self.db.insert_messages(self.account.id, folder_id, &older)?;
            fetched_total += older.len();
        }
        self.changed(Some(folder_id));
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
        self.touch();
        let id = self.db.upsert_folder(self.account.id, name, name, Some(role))?;
        Ok((id, name.to_string()))
    }

    /// Runs urgent commands that arrived while a long sync is in progress.
    async fn drain_urgent(&mut self) {
        loop {
            self.absorb_rx();
            let Some(cmd) = self.urgent.pop_front() else { break };
            if matches!(cmd, Cmd::FlushOps) {
                self.flush_queued = false;
            }
            self.handle_quick(cmd).await;
        }
    }

    async fn sync_all(&mut self, force: bool) -> Result<()> {
        self.status("syncing", None);
        // Replay any pending local mutations before listing/reconciling folders.
        let _ = self.flush_pending_ops(None).await?;

        let (use_list_status, condstore) = {
            let _ = self.ensure().await?;
            (self.caps.list_status, self.caps.condstore)
        };
        let s = self.session.as_mut().unwrap();
        let remote = imap::list_folders_with_status(s, use_list_status, condstore).await?;
        self.touch();

        let prev_ids: HashSet<i64> = self
            .db
            .account_folders(self.account.id)?
            .into_iter()
            .map(|(id, _, _)| id)
            .collect();
        let batch_items: Vec<(&str, &str, Option<&str>)> =
            remote.iter().map(|f| (f.path.as_str(), f.name.as_str(), f.role)).collect();
        let ids = self.db.upsert_folders_batch(self.account.id, &batch_items)?;
        let mut folders: Vec<(i64, String, Option<&'static str>, Option<FolderStatus>)> = remote
            .iter()
            .zip(ids)
            .map(|(f, id)| (id, f.path.clone(), f.role, f.status))
            .collect();
        let keep_ids: Vec<i64> = folders.iter().map(|f| f.0).collect();
        self.db.remove_folders_except(self.account.id, &keep_ids)?;
        let new_ids: HashSet<i64> = keep_ids.iter().copied().collect();
        if prev_ids != new_ids {
            self.changed(None);
        }

        let rank = |r: Option<&str>| match r {
            Some("inbox") => 0,
            Some("sent") => 1,
            Some("drafts") => 2,
            Some("archive") => 3,
            None => 4,
            _ => 5,
        };
        folders.sort_by_key(|f| rank(f.2));

        let mut synced_folders: Vec<(i64, String, Option<&'static str>)> = Vec::new();
        let mut folder_errors = 0usize;

        // Pass 1: headers & flags across changed folders.
        for (id, path, role, list_status) in &folders {
            let should_sync = self.folder_needs_sync(*id, path, *role, *list_status, force).await?;
            if should_sync {
                match self.sync_folder(*id, path, *role, force).await {
                    Ok(()) => synced_folders.push((*id, path.clone(), *role)),
                    Err(e) if is_folder_refusal_err(&e) => {
                        // Isolate single-folder refusals so Inbox/Sent/etc. still finish.
                        eprintln!("sync folder {path} skipped: {e:#}");
                        folder_errors += 1;
                    }
                    Err(e) => return Err(e),
                }
                self.drain_urgent().await;
            } else {
                // Still allow prefetching to top up bodies if earlier sync was interrupted.
                synced_folders.push((*id, path.clone(), *role));
            }
        }

        // Pass 2: byte-budgeted body prefetching.
        let mut stored_any = false;
        for (id, path, role) in &synced_folders {
            match self.prefetch(*id, path, *role, None).await {
                Ok(n) => stored_any |= n > 0,
                Err(e) if is_folder_refusal_err(&e) => {
                    eprintln!("prefetch {path} skipped: {e:#}");
                }
                Err(e) => return Err(e),
            }
        }

        // Keep per-account compressed body cache bounded on disk and reclaim pages when needed.
        if stored_any || force {
            let _ = self.db.prune_old_bodies(self.account.id, MAX_BODY_CACHE_BYTES);
        }

        if folder_errors > 0 && synced_folders.is_empty() {
            return Err(anyhow!("Couldn't sync any folders on the mail server"));
        }
        self.status("idle", None);
        Ok(())
    }

    /// Decides whether `folder_id` can be skipped using LIST-STATUS / STATUS watermarks.
    async fn folder_needs_sync(
        &mut self,
        folder_id: i64,
        path: &str,
        role: Option<&str>,
        list_status: Option<FolderStatus>,
        force: bool,
    ) -> Result<bool> {
        let state = self.db.folder_sync_state(folder_id)?;
        if !state.synced {
            return Ok(true);
        }
        if self.db.has_pending_ops_for_folder(folder_id)? {
            return Ok(true);
        }
        let now = now_secs();
        let full_reconcile_secs = match role {
            Some("inbox") => 300,
            Some("all") | Some("trash") | Some("junk") => 900,
            _ => 900,
        };

        // Resolve STATUS: either already bundled in LIST-STATUS, or queried via STATUS for non-selected folders.
        let st = match list_status {
            Some(s) => Some(s),
            None if !force && matches!(role, Some("all") | Some("trash") | Some("junk")) && now - state.last_full_sync < full_reconcile_secs => {
                // Without LIST-STATUS, don't hammer Trash/Junk/All Mail every 3 minutes on background polls.
                return Ok(false);
            }
            None if self.selected.as_deref() != Some(path) => {
                let condstore = self.caps.condstore;
                let s = self.ensure().await?;
                match imap::status(s, path, condstore).await {
                    Ok(s) => {
                        self.touch();
                        Some(s)
                    }
                    Err(_) => None,
                }
            }
            None => None,
        };

        let Some(st) = st else {
            return Ok(true);
        };
        if st.uid_validity != Some(state.uidvalidity) {
            return Ok(true);
        }
        if st.messages as i64 != state.msg_count {
            return Ok(true);
        }
        if st.uid_next != Some(state.uidnext) {
            return Ok(true);
        }
        // With CONDSTORE, HIGHESTMODSEQ catches every flag change too: exact 0-RTT skip!
        if state.highest_modseq > 0 && st.highest_modseq == Some(state.highest_modseq) {
            return Ok(false);
        }
        // Without CONDSTORE (or when server didn't report HIGHESTMODSEQ in STATUS),
        // skip when UNSEEN matches too until the periodic full flag check is due.
        if !force
            && st.unseen.map(|u| u as i64) == Some(state.unseen_count)
            && state.unseen_count >= 0
            && now - state.last_full_sync < full_reconcile_secs
        {
            return Ok(false);
        }
        Ok(true)
    }

    async fn sync_folder(&mut self, folder_id: i64, path: &str, role: Option<&str>, force: bool) -> Result<()> {
        // Flush any due pending mutations for this folder before reconciling with server state.
        let _ = self.flush_pending_ops(Some(folder_id)).await?;

        let condstore = self.caps.condstore;
        let s = self.ensure().await?;
        let mb = imap::select_folder(s, path, condstore).await?;
        self.touch();
        self.selected = Some(path.to_string());

        let mut prev_state = self.db.folder_sync_state(folder_id)?;
        let mut changed = false;
        if prev_state.uidvalidity != mb.uid_validity {
            self.db.reset_folder(folder_id, mb.uid_validity)?;
            prev_state = FolderSyncState { uidvalidity: mb.uid_validity, msg_count: -1, unseen_count: -1, ..Default::default() };
            changed = true;
        }
        let was_synced = prev_state.synced;
        let (pending_uids, mark_all_read_pending) = self.db.pending_uids_for_folder(folder_id)?;

        if mb.exists == 0 {
            let (c, _) = self.db.reconcile_flags_and_missing(folder_id, 0, &[], &pending_uids, mark_all_read_pending)?;
            changed |= c;
            self.db.update_folder_sync_state(
                folder_id,
                &FolderSyncState {
                    uidvalidity: mb.uid_validity,
                    synced: true,
                    uidnext: mb.uid_next.unwrap_or(1),
                    msg_count: 0,
                    unseen_count: 0,
                    highest_modseq: mb.highest_modseq.unwrap_or(0),
                    last_full_sync: now_secs(),
                },
            )?;
            if changed {
                self.changed(Some(folder_id));
            }
            return Ok(());
        }

        // Step 1: Fetch new headers (skipping completely when UIDNEXT proves nothing new arrived).
        let (_, max_uid) = self.db.uid_range(folder_id)?;
        let mut newly_added_ids: Vec<i64> = Vec::new();
        let mut server_new_count: u32 = 0;
        let prev_uidnext = prev_state.uidnext;

        if max_uid == 0 {
            let start = mb.exists.saturating_sub(INITIAL_HEADERS - 1).max(1);
            let mut cur = start;
            while cur <= mb.exists {
                let end = (cur + UID_CHUNK as u32 - 1).min(mb.exists);
                let s = self.session.as_mut().unwrap();
                let hdrs = imap::fetch_headers(s, &format!("{cur}:{end}"), false).await?;
                self.touch();
                let new = to_new_messages(&hdrs, |uid| uid > max_uid);
                if !new.is_empty() {
                    let added = self.db.insert_messages(self.account.id, folder_id, &new)?;
                    newly_added_ids.extend(added);
                    changed = true;
                }
                cur = end.saturating_add(1);
            }
        } else if mb.uid_next.is_none_or(|next| next > max_uid.saturating_add(1)) {
            if let Some(next_uid) = max_uid.checked_add(1) {
                let direct_range = mb.uid_next.is_some_and(|next| next.saturating_sub(next_uid) <= UID_CHUNK as u32);
                if direct_range {
                    let s = self.session.as_mut().unwrap();
                    let hdrs = imap::fetch_headers(s, &format!("{next_uid}:*"), true).await?;
                    self.touch();
                    server_new_count = hdrs.iter().filter(|h| h.uid >= prev_uidnext.max(next_uid)).count() as u32;
                    let new = to_new_messages(&hdrs, |uid| uid > max_uid);
                    if !new.is_empty() {
                        let added = self.db.insert_messages(self.account.id, folder_id, &new)?;
                        newly_added_ids.extend(added);
                        changed = true;
                    }
                } else {
                    // Large catch-up gap: find exact new UIDs first, then fetch in bounded chunks.
                    let s = self.session.as_mut().unwrap();
                    let mut new_uids = imap::uid_search(s, &format!("UID {next_uid}:*")).await?;
                    self.touch();
                    new_uids.retain(|&u| u > max_uid);
                    new_uids.sort_unstable();
                    server_new_count = new_uids.iter().filter(|&&u| u >= prev_uidnext.max(next_uid)).count() as u32;
                    for chunk in new_uids.chunks(UID_CHUNK) {
                        let s = self.session.as_mut().unwrap();
                        let hdrs = imap::fetch_headers(s, &imap::uid_set(chunk), true).await?;
                        self.touch();
                        let new = to_new_messages(&hdrs, |uid| uid > max_uid);
                        if !new.is_empty() {
                            let added = self.db.insert_messages(self.account.id, folder_id, &new)?;
                            newly_added_ids.extend(added);
                            changed = true;
                        }
                    }
                }
            }
        }

        // Step 2: Reconcile flags, deletions, and missing UIDs for everything in the local window.
        let (min_uid, new_max_uid) = self.db.uid_range(folder_id)?;
        let mut did_full_reconcile = false;
        if min_uid > 0 {
            // Exact expunge invariant: since UIDs are strictly ascending,
            // server_exists == prev_exists + newly_added_still_present iff 0 older messages were expunged.
            let no_expunges = was_synced
                && prev_state.msg_count >= 0
                && prev_uidnext > 0
                && !force
                && mb.exists == (prev_state.msg_count as u32).saturating_add(server_new_count);

            let can_condstore = self.caps.condstore
                && !mb.nomodseq
                && prev_state.highest_modseq > 0
                && mb.highest_modseq.is_some()
                && !force;

            let mut missing_to_heal: Vec<u32> = Vec::new();
            if can_condstore {
                let s = self.session.as_mut().unwrap();
                if mb.highest_modseq != Some(prev_state.highest_modseq) {
                    let changed_flags =
                        imap::fetch_flags_since(s, &format!("{min_uid}:*"), prev_state.highest_modseq).await?;
                    self.touch();
                    changed |= self.db.apply_changed_flags(folder_id, &changed_flags, &pending_uids, mark_all_read_pending)?;
                }
                if !no_expunges {
                    // Compact UID SEARCH detects expunges & missing UIDs without downloading flags for every message.
                    let s = self.session.as_mut().unwrap();
                    let present: HashSet<u32> = imap::uid_search(s, &format!("UID {min_uid}:*")).await?.into_iter().collect();
                    self.touch();
                    if present.is_empty() {
                        return Err(anyhow!("The mail server's answer was cut off (no UIDs listed)"));
                    }
                    let (c, missing) = self.db.reconcile_existing_uids(folder_id, min_uid, &present, &pending_uids)?;
                    changed |= c;
                    missing_to_heal = missing;
                }
                did_full_reconcile = true;
            } else {
                let s = self.session.as_mut().unwrap();
                let flags = imap::fetch_flags(s, format!("{min_uid}:*"), true).await?;
                self.touch();
                let (c, missing) =
                    self.db.reconcile_flags_and_missing(folder_id, min_uid, &flags, &pending_uids, mark_all_read_pending)?;
                changed |= c;
                missing_to_heal = missing;
                did_full_reconcile = true;
            }

            // Heal any server UIDs in the synced window missing locally (skipping pending local deletes/moves).
            if !missing_to_heal.is_empty() {
                missing_to_heal.sort_unstable_by(|a, b| b.cmp(a));
                missing_to_heal.truncate(BACKFILL);
                for chunk in missing_to_heal.chunks(UID_CHUNK) {
                    let s = self.session.as_mut().unwrap();
                    let hdrs = imap::fetch_headers(s, &imap::uid_set(chunk), true).await?;
                    self.touch();
                    let restored = to_new_messages(&hdrs, |u| chunk.contains(&u));
                    if !self.db.insert_messages(self.account.id, folder_id, &restored)?.is_empty() {
                        changed = true;
                    }
                }
            }
        }

        // Step 3: Search hits below the window: check just those.
        let sparse = self.db.sparse_uids_below(folder_id, if min_uid > 0 { min_uid } else { u32::MAX })?;
        for chunk in sparse.chunks(UID_CHUNK) {
            let s = self.session.as_mut().unwrap();
            let flags = imap::fetch_flags(s, imap::uid_set(chunk), false).await?;
            self.touch();
            changed |= self.db.reconcile_uids(folder_id, chunk, &flags, &pending_uids)?;
        }

        // Persist updated folder watermarks.
        let next_uidnext = mb.uid_next.unwrap_or_else(|| new_max_uid.saturating_add(1));
        let next_modseq = if mb.nomodseq { 0 } else { mb.highest_modseq.unwrap_or(prev_state.highest_modseq) };
        let local_unseen = self.db.folder_unread(folder_id).unwrap_or(-1);
        self.db.update_folder_sync_state(
            folder_id,
            &FolderSyncState {
                uidvalidity: mb.uid_validity,
                synced: true,
                uidnext: next_uidnext,
                msg_count: if pending_uids.is_empty() { mb.exists as i64 } else { -1 },
                unseen_count: if pending_uids.is_empty() && !mark_all_read_pending { local_unseen } else { -1 },
                highest_modseq: next_modseq,
                last_full_sync: if did_full_reconcile { now_secs() } else { prev_state.last_full_sync },
            },
        )?;

        // Step 4: Queue newly arrived Inbox mail for rules AFTER reconciliation completes,
        // so rule moves/flag changes never race with `reconcile_flags` on the same sync pass.
        if role == Some("inbox") && was_synced && !newly_added_ids.is_empty() {
            self.db.queue_for_rules(self.account.id, &newly_added_ids, now_secs())?;
            self.rules_wake.notify_one();
        }

        if changed {
            self.changed(Some(folder_id));
        }
        Ok(())
    }

    /// Downloads recent bodies ahead of time so opening a message is instant.
    /// Deduplicates locally across folders/stashes first, packs chunks by byte budget,
    /// parses MIME off the async runtime, and commits each chunk in one transaction.
    /// Coalesces UI change events so prefetching never floods the frontend every 15 messages.
    async fn prefetch(&mut self, folder_id: i64, path: &str, role: Option<&str>, limit_override: Option<usize>) -> Result<usize> {
        let (default_limit, max_size) = prefetch_policy(role);
        let limit = limit_override.map(|l| l.min(default_limit)).unwrap_or(default_limit);
        if limit == 0 {
            return Ok(0);
        }
        // Hydrate any bodies already cached in another folder (Gmail labels) or in `moved_bodies`.
        let mut total_stored = 0usize;
        let hydrated = self.db.copy_existing_bodies_for_folder(self.account.id, folder_id)?;
        if hydrated > 0 {
            self.changed(Some(folder_id));
        }
        let missing = self.db.missing_bodies_with_size(folder_id, limit, max_size)?;
        if missing.is_empty() {
            return Ok(total_stored);
        }
        let batches = pack_prefetch_batches(&missing, PREFETCH_MAX_COUNT, PREFETCH_MAX_BYTES);
        let mut pending_notify = false;
        for (idx, chunk) in batches.into_iter().enumerate() {
            self.select(path).await?;
            let s = self.ensure().await?;
            let fetched = imap::fetch_bodies(s, &imap::uid_set(&chunk)).await?;
            self.touch();
            if !fetched.is_empty() {
                let parsed = tokio::task::spawn_blocking(move || {
                    fetched
                        .into_iter()
                        .map(|(uid, raw)| {
                            let info = parse::body_info(&raw);
                            (uid, raw, info)
                        })
                        .collect::<Vec<_>>()
                })
                .await
                .map_err(|_| anyhow!("Parsing bodies failed"))?;
                let stored = self.db.store_bodies_batch(folder_id, &parsed)?;
                if stored > 0 {
                    total_stored += stored;
                    if idx == 0 {
                        // Emit once after the newest visible chunk so top snippets appear right away.
                        self.changed(Some(folder_id));
                        pending_notify = false;
                    } else {
                        pending_notify = true;
                    }
                }
            }
            self.drain_urgent().await;
        }
        if pending_notify {
            self.changed(Some(folder_id));
        }
        Ok(total_stored)
    }
}

/// Per-folder background body prefetch policy: `(max_messages, max_email_bytes)`.
/// Skips Trash, Junk, and Gmail's All Mail (which duplicates Inbox/Sent/Archive);
/// caps custom folders lightly so large accounts stay fast and lean on disk.
fn prefetch_policy(role: Option<&str>) -> (usize, u32) {
    match role {
        Some("trash") | Some("junk") | Some("all") => (0, 0),
        Some("inbox") => (250, PREFETCH_MAX_EMAIL_BYTES),
        Some("sent") | Some("drafts") | Some("snoozed") | Some("archive") => (80, PREFETCH_MAX_EMAIL_BYTES),
        None => (25, PREFETCH_MAX_EMAIL_BYTES),
        _ => (40, PREFETCH_MAX_EMAIL_BYTES),
    }
}

/// Packs `(uid, size)` items into batches capped by both count and cumulative byte budget.
fn pack_prefetch_batches(items: &[(u32, u32)], max_count: usize, max_bytes: u32) -> Vec<Vec<u32>> {
    let mut batches = Vec::new();
    let mut cur = Vec::new();
    let mut bytes = 0u32;
    for &(uid, size) in items {
        if !cur.is_empty() && (cur.len() >= max_count || bytes.saturating_add(size) > max_bytes) {
            batches.push(std::mem::take(&mut cur));
            bytes = 0;
        }
        cur.push(uid);
        bytes = bytes.saturating_add(size);
    }
    if !cur.is_empty() {
        batches.push(cur);
    }
    batches
}

fn to_new_messages(fetches: &[FetchedHeader], keep: impl Fn(u32) -> bool) -> Vec<NewMessage> {
    fetches
        .iter()
        .filter(|f| keep(f.uid))
        .map(|f| parse::parse_header(f.uid, &f.header, f.seen, f.flagged, f.size, f.internal_date))
        .collect()
}

fn is_not_found_err(e: &anyhow::Error) -> bool {
    format!("{e:#}").contains("Message no longer on server")
}

fn is_folder_refusal_err(e: &anyhow::Error) -> bool {
    let msg = format!("{e:#}");
    msg.contains("(No:") && !msg.contains("server closed") && !msg.contains("Timed out")
}

fn is_permanent_op_err(e: &anyhow::Error) -> bool {
    let msg = format!("{e:#}");
    msg.contains("Query returned no rows") || msg.contains("No ") && msg.contains(" folder")
}

/// Holds a read-only IDLE connection on the inbox and asks the worker to sync on external changes.
pub async fn idle_loop(
    account: Account,
    db: Arc<Db>,
    last_inbox_write_ms: Arc<AtomicI64>,
    tx: mpsc::UnboundedSender<Cmd>,
) {
    let mut backoff = 5u64;
    let mut cached_password: Option<String> = None;
    let mut has_connected_before = false;

    loop {
        let started = Instant::now();
        match idle_once(&account, &db, &last_inbox_write_ms, &tx, &mut cached_password, &mut has_connected_before).await {
            Ok(()) => return, // worker gone or server doesn't support IDLE
            Err(e) => {
                if format!("{e:#}").contains("Sign-in failed") {
                    cached_password = None;
                }
                if started.elapsed() >= Duration::from_secs(30) {
                    backoff = 5;
                }
                if tx.is_closed() {
                    return;
                }
                tokio::time::sleep(Duration::from_secs(backoff)).await;
                backoff = (backoff * 2).min(300);
            }
        }
    }
}

async fn idle_once(
    account: &Account,
    db: &Db,
    last_inbox_write_ms: &AtomicI64,
    tx: &mpsc::UnboundedSender<Cmd>,
    cached_password: &mut Option<String>,
    has_connected_before: &mut bool,
) -> Result<()> {
    let pw = match cached_password {
        Some(p) => p.clone(),
        None => {
            let p = secrets::get(&account.email)?;
            *cached_password = Some(p.clone());
            p
        }
    };
    let mut session = imap::connect(&account.imap_host, account.imap_port, &account.email, &pw).await?;
    let caps = imap::capabilities(&mut session).await.unwrap_or_default();
    if !caps.idle {
        // Server does not advertise RFC 2177 IDLE: exit cleanly and let the 180s poll loop handle syncing.
        let _ = session.logout().await;
        return Ok(());
    }

    let inbox_path = db
        .folder_by_role(account.id, "inbox")?
        .map(|(_, p)| p)
        .unwrap_or_else(|| "INBOX".to_string());
    // Read-only EXAMINE avoids interfering with the worker's RW SELECT or \Recent flags.
    imap::examine(&mut session, &inbox_path).await?;

    // After a disconnect/sleep/network switch, trigger a catch-up sync so mail received while
    // offline isn't delayed until the next 3-minute poll.
    if *has_connected_before {
        let _ = tx.send(Cmd::SyncAll { force: false });
    }
    *has_connected_before = true;

    loop {
        let mut idle = session.idle();
        timed(CMD_TIMEOUT, "starting IDLE", idle.init()).await?;
        let res = {
            let (wait, _stop) = idle.wait_with_timeout(IDLE_REFRESH);
            wait.await?
        };
        session = timed(CMD_TIMEOUT, "ending IDLE", idle.done()).await?;
        if let IdleResponse::NewData(data) = res {
            let is_echo = match data.parsed() {
                // New arrivals (EXISTS / RECENT) always trigger a sync immediately.
                Response::MailboxData(MailboxDatum::Exists(_) | MailboxDatum::Recent(_)) => false,
                // Flag/expunge notifications right after our own Inbox mutation are self-echoes.
                Response::Fetch(..) | Response::Expunge(_) => {
                    now_ms().saturating_sub(last_inbox_write_ms.load(Ordering::Relaxed)) < ECHO_SUPPRESS_MS
                }
                _ => false,
            };
            if !is_echo {
                let cmd = match db.folder_by_role(account.id, "inbox")? {
                    Some((folder_id, _)) => Cmd::SyncFolder { folder_id, from_idle: true },
                    None => Cmd::SyncAll { force: false },
                };
                if tx.send(cmd).is_err() {
                    return Ok(());
                }
            }
        }
        if tx.is_closed() {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefetch_policy_skips_trash_and_junk_and_caps_at_4mb() {
        assert_eq!(prefetch_policy(Some("trash")), (0, 0));
        assert_eq!(prefetch_policy(Some("junk")), (0, 0));
        assert_eq!(prefetch_policy(Some("all")), (0, 0));
        for role in [Some("inbox"), Some("sent"), Some("drafts"), Some("snoozed"), Some("archive"), None] {
            let (limit, max_bytes) = prefetch_policy(role);
            assert!(limit > 0, "expected prefetch enabled for {role:?}");
            assert_eq!(max_bytes, 4 * 1024 * 1024);
        }
    }

    #[test]
    fn prefetch_batches_respect_both_count_and_byte_budget() {
        // Three 3MB messages: each exceeds half of 4MB, so each gets its own batch.
        let items = vec![(1, 3_000_000), (2, 3_000_000), (3, 1_000_000), (4, 1_000_000), (5, 1_000_000)];
        let b = pack_prefetch_batches(&items, 15, 4_000_000);
        assert_eq!(b, vec![vec![1], vec![2, 3], vec![4, 5]]);

        // Single huge message above budget still makes progress solo.
        let huge = vec![(10, 7_500_000), (11, 50_000)];
        assert_eq!(pack_prefetch_batches(&huge, 15, 4_000_000), vec![vec![10], vec![11]]);

        // Many tiny messages respect max_count.
        let tiny: Vec<(u32, u32)> = (1..=35).map(|u| (u, 10_000)).collect();
        let b = pack_prefetch_batches(&tiny, 15, 4_000_000);
        assert_eq!(b.iter().map(Vec::len).collect::<Vec<_>>(), vec![15, 15, 5]);
    }

    #[test]
    fn sync_queue_coalesces_duplicates_and_prioritizes_urgent() {
        let dir = std::env::temp_dir().join(format!("mailvern-worker-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Arc::new(Db::open(&dir.join("mailvern.db")).unwrap());
        let acc = Account {
            id: 1,
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
            ..Default::default()
        };
        let (ev_tx, _) = broadcast::channel(8);
        let (tx, rx) = mpsc::unbounded_channel();
        let mut w = Worker::new(acc, db, ev_tx, Arc::new(Notify::new()), Arc::new(AtomicI64::new(0)), rx);

        // Multiple IDLE syncs for same folder collapse into 1; manual overrides from_idle.
        tx.send(Cmd::SyncFolder { folder_id: 5, from_idle: true }).unwrap();
        tx.send(Cmd::SyncFolder { folder_id: 5, from_idle: true }).unwrap();
        tx.send(Cmd::SyncFolder { folder_id: 5, from_idle: false }).unwrap();
        // Multiple FlushOps collapse into 1 and run BEFORE SyncFolder.
        tx.send(Cmd::FlushOps).unwrap();
        tx.send(Cmd::FlushOps).unwrap();

        assert!(matches!(w.next_pending(), Some(Cmd::FlushOps)));
        assert!(matches!(w.next_pending(), Some(Cmd::SyncFolder { folder_id: 5, from_idle: false })));
        assert!(w.next_pending().is_none());

        // SyncAll collapses SyncFolder and merges force=true.
        tx.send(Cmd::SyncFolder { folder_id: 5, from_idle: true }).unwrap();
        tx.send(Cmd::SyncAll { force: false }).unwrap();
        tx.send(Cmd::SyncFolder { folder_id: 6, from_idle: true }).unwrap();
        tx.send(Cmd::SyncAll { force: true }).unwrap();
        assert!(matches!(w.next_pending(), Some(Cmd::SyncAll { force: true })));
        assert!(w.next_pending().is_none());
    }
}
