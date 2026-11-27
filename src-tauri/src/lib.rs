use mail_core::model::{
    Account, AttachmentContent, BootstrapData, Folder, MailGroup, MailQuery, MessageDetail, MessageSummary, Outgoing, OutboxItem, Rule,
    RuleActivity, RuleRun, RuleTest, SearchScope, SmartCounts, View,
};
use mail_core::text::{quarantine_file, safe_filename};
use mail_core::Engine;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{Emitter, Manager, State};
use tauri_plugin_opener::OpenerExt;

mod agents;
mod ai_http;
#[cfg(target_os = "macos")]
mod context_menu;

type E<'a> = State<'a, Arc<Engine>>;
type R<T> = Result<T, String>;

// Plain commands run on the main thread, which also delivers keystrokes to the window.
// Commands marked `(async)` run on Tauri's worker pool so SQLite, Keychain, or file I/O
// never stalls typing or window responsiveness.

fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

#[tauri::command(async)]
fn accounts(engine: E) -> R<Vec<Account>> {
    engine.accounts().map_err(err)
}

#[tauri::command]
async fn save_account(engine: E<'_>, account: Account, password: Option<String>) -> R<Account> {
    engine.save_account(account, password).await.map_err(err)
}

#[tauri::command(async)]
fn delete_account(engine: E, id: i64) -> R<()> {
    engine.delete_account(id).map_err(err)
}

#[tauri::command(async)]
fn reorder_accounts(engine: E, ids: Vec<i64>) -> R<()> {
    engine.reorder_accounts(&ids).map_err(err)
}

#[tauri::command(async)]
fn set_account_alerts(engine: E, account_id: i64, notify: bool, badge_mode: String) -> R<()> {
    engine.set_account_alerts(account_id, notify, &badge_mode).map_err(err)
}

#[tauri::command(async)]
fn reorder_folders(engine: E, account_id: i64, ids: Vec<i64>) -> R<()> {
    engine.reorder_folders(account_id, &ids).map_err(err)
}

#[tauri::command]
async fn create_folder(engine: E<'_>, account_id: i64, name: String, parent_id: Option<i64>) -> R<Folder> {
    engine.create_folder(account_id, &name, parent_id).await.map_err(err)
}

#[tauri::command]
async fn rename_folder(engine: E<'_>, folder_id: i64, new_name: String) -> R<()> {
    engine.rename_folder(folder_id, &new_name).await.map_err(err)
}

#[tauri::command]
async fn delete_folder(engine: E<'_>, folder_id: i64) -> R<()> {
    engine.delete_folder(folder_id).await.map_err(err)
}

#[tauri::command(async)]
fn empty_folder(engine: E, view: View) -> R<()> {
    engine.empty_folder(&view).map_err(err)
}

#[tauri::command(async)]
fn permanent_delete(engine: E, ids: Vec<i64>) -> R<()> {
    engine.permanent_delete(&ids).map_err(err)
}

#[tauri::command(async)]
fn folders(engine: E) -> R<Vec<Folder>> {
    engine.folders().map_err(err)
}

#[tauri::command(async)]
fn smart_counts(engine: E) -> R<SmartCounts> {
    engine.smart_counts().map_err(err)
}

#[tauri::command(async)]
fn messages(engine: E, view: View, query: Option<String>, offset: i64, limit: i64, threaded: Option<bool>) -> R<Vec<MessageSummary>> {
    engine.messages(&view, query.as_deref(), offset, limit, threaded.unwrap_or(true)).map_err(err)
}

#[tauri::command(async)]
fn thread(engine: E, id: i64) -> R<Vec<MessageSummary>> {
    engine.thread(id).map_err(err)
}

#[tauri::command]
async fn search_server(engine: E<'_>, query: String, scope: Option<SearchScope>) -> R<Vec<MessageSummary>> {
    engine.search_server(&query, &scope.unwrap_or_default()).await.map_err(err)
}

#[tauri::command(async)]
fn move_to(engine: E, ids: Vec<i64>, folder_id: i64) -> R<()> {
    engine.move_to(&ids, folder_id).map_err(err)
}

#[tauri::command(async)]
fn snooze(engine: E, ids: Vec<i64>, until: i64) -> R<()> {
    engine.snooze(&ids, until).map_err(err)
}

#[tauri::command(async)]
fn unsnooze(engine: E, ids: Vec<i64>) -> R<()> {
    engine.unsnooze(&ids).map_err(err)
}

#[tauri::command]
async fn load_older(engine: E<'_>, view: View) -> R<usize> {
    engine.load_older(&view).await.map_err(err)
}

#[tauri::command(async)]
fn search(engine: E, query: String, scope: Option<SearchScope>) -> R<Vec<MessageSummary>> {
    engine.search(&query, &scope.unwrap_or_default()).map_err(err)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct QueryResult {
    total: i64,
    rows: Vec<MessageSummary>,
    /// Set when `server` was asked for and every server failed (local results still come back).
    server_error: Option<String>,
}

/// Structured search for the AI tools. With `server`, the mail servers are asked first.
#[tauri::command]
async fn query_mail(engine: E<'_>, query: MailQuery, server: Option<bool>) -> R<QueryResult> {
    let (r, server_error) = if server.unwrap_or(false) {
        engine.query_mail_server(&query).await.map_err(err)?
    } else {
        (engine.query_mail(&query).map_err(err)?, None)
    };
    Ok(QueryResult { total: r.total, rows: r.rows, server_error })
}

#[derive(serde::Serialize)]
struct GroupResult {
    total: i64,
    groups: Vec<MailGroup>,
}

#[tauri::command(async)]
fn group_mail(engine: E, query: MailQuery, by: String) -> R<GroupResult> {
    let (total, groups) = engine.group_mail(&query, &by).map_err(err)?;
    Ok(GroupResult { total, groups })
}

#[tauri::command(async)]
fn summaries(engine: E, ids: Vec<i64>) -> R<Vec<MessageSummary>> {
    engine.summaries(&ids).map_err(err)
}

#[tauri::command]
async fn attachment_content(engine: E<'_>, id: i64, index: usize, offset: Option<usize>, max_chars: Option<usize>) -> R<AttachmentContent> {
    let max = max_chars.unwrap_or(20_000).clamp(500, 200_000);
    engine.attachment_content(id, index, offset.unwrap_or(0), max).await.map_err(err)
}

/// The legacy OpenRouter Keychain entry name (kept for backwards compatibility).
const OPENROUTER_KEY: &str = "openrouter-api-key";
/// Non-secret flag in SQLite settings so startup avoids a Keychain IPC lookup.
const AI_CONFIGURED: &str = "ai_configured";

fn ai_keychain_name(provider: Option<&str>) -> R<String> {
    let p = provider.unwrap_or("openrouter").trim();
    if p.is_empty() || p == "openrouter" {
        return Ok(OPENROUTER_KEY.to_string());
    }
    if p.len() > 64 || !p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return Err("Invalid AI provider id".into());
    }
    Ok(format!("ai-api-key-{p}"))
}

#[tauri::command(async)]
fn ai_key(engine: E, provider: Option<String>) -> R<Option<String>> {
    let name = ai_keychain_name(provider.as_deref())?;
    let key = mail_core::secrets::get(&name).ok().filter(|k| !k.is_empty());
    let _ = engine.set_setting(AI_CONFIGURED, if key.is_some() { "1" } else { "0" });
    Ok(key)
}

/// Stores the key for a provider, or removes it when `key` is empty.
#[tauri::command(async)]
fn set_ai_key(engine: E, key: String, provider: Option<String>) -> R<()> {
    let name = ai_keychain_name(provider.as_deref())?;
    let key = key.trim();
    if key.is_empty() {
        mail_core::secrets::delete(&name);
        let _ = engine.set_setting(AI_CONFIGURED, "0");
        Ok(())
    } else {
        mail_core::secrets::set(&name, key).map_err(err)?;
        let _ = engine.set_setting(AI_CONFIGURED, "1");
        Ok(())
    }
}

/// Initial inbox page size precomputed during parallel preboot (covers ~4 viewports).
const BOOT_PAGE_LIMIT: i64 = 40;

const STARTUP_SETTING_KEYS: &[&str] = &[
    "theme",
    "undo_send",
    "threads",
    "sender_icons",
    "side_width",
    "list_width",
    "sidebar_collapsed",
    "remote_images",
    "auto_trust_sent",
    "trusted_senders",
    "shortcuts",
    "badge_mode",
    "ai_provider",
    "ai_search_model",
    "ai_write_model",
    "ai_private",
    "ai_configured",
];

/// Holds a single-use `BootstrapData` computed on a background thread while WKWebView spawns.
struct PrebootBootstrap {
    rx: std::sync::Mutex<Option<std::sync::mpsc::Receiver<R<BootstrapData>>>>,
}

impl PrebootBootstrap {
    fn new(rx: std::sync::mpsc::Receiver<R<BootstrapData>>) -> Self {
        Self { rx: std::sync::Mutex::new(Some(rx)) }
    }

    fn empty() -> Self {
        Self { rx: std::sync::Mutex::new(None) }
    }

    fn take(&self) -> Option<R<BootstrapData>> {
        let rx = self.rx.lock().unwrap_or_else(|e| e.into_inner()).take()?;
        rx.recv().ok()
    }
}

/// Releases background account syncing & MCP startup once the first bootstrap snapshot has been delivered.
struct StartGate {
    opened: std::sync::atomic::AtomicBool,
    notify: tokio::sync::Notify,
}

impl StartGate {
    fn new() -> Self {
        Self {
            opened: std::sync::atomic::AtomicBool::new(false),
            notify: tokio::sync::Notify::new(),
        }
    }

    fn open(&self) {
        if !self.opened.swap(true, std::sync::atomic::Ordering::SeqCst) {
            self.notify.notify_waiters();
        }
    }

    async fn wait(&self) {
        if self.opened.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let notified = self.notify.notified();
        if self.opened.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        notified.await;
    }
}

#[tauri::command(async)]
fn bootstrap(
    engine: E,
    preboot: State<'_, PrebootBootstrap>,
    start_gate: State<'_, Arc<StartGate>>,
    view: View,
    limit: i64,
    setting_keys: Vec<String>,
) -> R<BootstrapData> {
    let is_default_boot = matches!(&view, View::Unified { role } if role == "inbox") && limit == BOOT_PAGE_LIMIT;
    let res = if is_default_boot {
        if let Some(pre) = preboot.take() {
            pre
        } else {
            let keys: Vec<&str> = setting_keys.iter().map(String::as_str).filter(|k| !is_restricted_setting(k)).collect();
            engine.bootstrap(&view, limit, &keys).map_err(err)
        }
    } else {
        let keys: Vec<&str> = setting_keys.iter().map(String::as_str).filter(|k| !is_restricted_setting(k)).collect();
        engine.bootstrap(&view, limit, &keys).map_err(err)
    };
    start_gate.open();
    res
}

#[tauri::command(async)]
fn get_settings(engine: E, keys: Vec<String>) -> R<HashMap<String, String>> {
    let allowed: Vec<&str> = keys.iter().map(String::as_str).filter(|k| !is_restricted_setting(k)).collect();
    engine.settings_many(&allowed).map_err(err)
}

#[tauri::command(async)]
fn badge_count(engine: E) -> R<i64> {
    engine.badge_count().map_err(err)
}

#[tauri::command(async)]
fn mark_badge_seen(engine: E) -> R<()> {
    engine.mark_badge_seen().map_err(err)
}

#[tauri::command]
async fn message(engine: E<'_>, id: i64) -> R<MessageDetail> {
    engine.message(id).await.map_err(err)
}

#[tauri::command]
async fn rsvp_invite(engine: E<'_>, id: i64, status: String) -> R<String> {
    engine.rsvp_invite(id, &status).await.map_err(err)
}

#[tauri::command]
async fn open_ics(app: tauri::AppHandle, engine: E<'_>, id: i64) -> R<()> {
    let (name, bytes) = engine.raw_ics(id).await.map_err(err)?;
    let path = tokio::task::spawn_blocking(move || -> R<PathBuf> {
        let dir = std::env::temp_dir().join("Mailvern").join(format!("ics-{id}"));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = write_unique_file(&dir, &safe_filename(&name), &bytes)?;
        quarantine_file(&path);
        Ok(path)
    })
    .await
    .map_err(|_| "Writing invite failed".to_string())??;
    app.opener().open_path(path.to_string_lossy(), None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
async fn raw_source(engine: E<'_>, id: i64) -> R<String> {
    let (_, raw) = engine.raw_message(id).await.map_err(err)?;
    tokio::task::spawn_blocking(move || String::from_utf8_lossy(&raw).into_owned())
        .await
        .map_err(|_| "Decoding message source failed".to_string())
}

#[tauri::command]
async fn export_eml(app: tauri::AppHandle, engine: E<'_>, id: i64, dest: Option<String>) -> R<String> {
    let (name, raw) = engine.raw_message(id).await.map_err(err)?;
    let path = tokio::task::spawn_blocking(move || -> R<PathBuf> {
        if let Some(d) = dest.filter(|s| !s.trim().is_empty()) {
            let p = PathBuf::from(d);
            std::fs::write(&p, &raw).map_err(|e| e.to_string())?;
            Ok(p)
        } else {
            let dir = dirs::download_dir().ok_or("No Downloads folder")?;
            write_unique_file(&dir, &safe_filename(&name), &raw)
        }
    })
    .await
    .map_err(|_| "Exporting message failed".to_string())??;
    let _ = app.opener().reveal_item_in_dir(&path);
    Ok(path.to_string_lossy().into())
}

#[tauri::command(async)]
fn cached_message(engine: E, id: i64) -> R<Option<MessageDetail>> {
    engine.cached_message(id).map_err(err)
}

#[tauri::command(async)]
fn set_seen(engine: E, ids: Vec<i64>, seen: bool) -> R<()> {
    engine.set_seen(&ids, seen).map_err(err)
}

#[tauri::command(async)]
fn mark_read(engine: E, view: View) -> R<()> {
    engine.mark_read(&view).map_err(err)
}

#[tauri::command(async)]
fn set_flagged(engine: E, ids: Vec<i64>, flagged: bool) -> R<()> {
    engine.set_flagged(&ids, flagged).map_err(err)
}

#[tauri::command(async)]
fn archive(engine: E, ids: Vec<i64>) -> R<()> {
    engine.archive(&ids).map_err(err)
}

#[tauri::command(async)]
fn trash(engine: E, ids: Vec<i64>) -> R<()> {
    engine.trash(&ids).map_err(err)
}

#[tauri::command(async)]
fn junk(engine: E, ids: Vec<i64>) -> R<()> {
    engine.junk(&ids).map_err(err)
}

#[tauri::command(async)]
fn not_junk(engine: E, ids: Vec<i64>) -> R<()> {
    engine.not_junk(&ids).map_err(err)
}

/// Queues a message: after the undo window, or at `send_at` (unix seconds). Returns its outbox id.
#[tauri::command(async)]
fn send(engine: E, message: Outgoing, send_at: Option<i64>) -> R<i64> {
    engine.send(message, send_at).map_err(err)
}

#[derive(serde::Serialize)]
struct Cancelled {
    message: Outgoing,
    attachments: Vec<FileInfo>,
}

/// Takes a message back out of the outbox to edit it. Its attachments are written to a
/// temp folder so the composer can attach them again.
#[tauri::command(async)]
fn cancel_send(engine: E, id: i64) -> R<Cancelled> {
    let (message, files) = engine.cancel_send(id).map_err(err)?;
    let dir = std::env::temp_dir().join("Mailvern").join(format!("outbox-{id}"));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut attachments = Vec::new();
    for (name, bytes) in files {
        let safe = safe_filename(&name);
        let path = write_unique_file(&dir, &safe, &bytes)?;
        attachments.push(FileInfo { path: path.to_string_lossy().into(), name: safe, size: bytes.len() as u64 });
    }
    Ok(Cancelled { message, attachments })
}

#[tauri::command(async)]
fn send_now(engine: E, id: i64) -> R<()> {
    engine.send_now(id).map_err(err)
}

#[tauri::command(async)]
fn outbox(engine: E) -> R<Vec<OutboxItem>> {
    engine.outbox().map_err(err)
}

#[tauri::command]
async fn save_draft(engine: E<'_>, message: Outgoing) -> R<String> {
    engine.save_draft(message).await.map_err(err)
}

#[tauri::command(async)]
fn delete_draft(engine: E, account_id: i64, message_id: String) -> R<()> {
    engine.delete_draft(account_id, &message_id).map_err(err)
}

#[tauri::command(async)]
fn sync_now(engine: E) {
    engine.sync_now();
}

#[tauri::command(async)]
fn sync_account(engine: E, account_id: i64) -> R<()> {
    engine.sync_account(account_id).map_err(err)
}

#[tauri::command(async)]
fn sync_view(engine: E, view: View) -> R<()> {
    engine.sync_view(&view).map_err(err)
}

fn is_restricted_setting(key: &str) -> bool {
    matches!(key, agents::ENABLED | agents::MODE | agents::LEGACY_TOKEN)
}

#[tauri::command(async)]
fn get_setting(engine: E, key: String) -> R<Option<String>> {
    if is_restricted_setting(&key) {
        return Err("That setting can only be managed from Agent Settings".into());
    }
    engine.setting(&key).map_err(err)
}

#[tauri::command(async)]
fn set_setting(engine: E, key: String, value: String) -> R<()> {
    if is_restricted_setting(&key) {
        return Err("That setting can only be managed from Agent Settings".into());
    }
    engine.set_setting(&key, &value).map_err(err)
}

// ---------- rules ----------

#[tauri::command(async)]
fn rules(engine: E) -> R<Vec<Rule>> {
    engine.rules().map_err(err)
}

/// Saves a rule from the Rules settings: the user's own, so it's live at once.
#[tauri::command(async)]
fn save_rule(engine: E, rule: Rule) -> R<Rule> {
    engine.save_rule(rule, false).map_err(err)
}

#[tauri::command(async)]
fn delete_rule(engine: E, id: i64) -> R<()> {
    engine.delete_rule(id).map_err(err)
}

#[tauri::command(async)]
fn reorder_rules(engine: E, ids: Vec<i64>) -> R<()> {
    engine.reorder_rules(&ids).map_err(err)
}

#[tauri::command(async)]
fn set_rule_enabled(engine: E, id: i64, enabled: bool) -> R<()> {
    engine.set_rule_enabled(id, enabled).map_err(err)
}

#[tauri::command(async)]
fn check_rule(engine: E, rule: Rule) -> R<()> {
    engine.check_rule(&rule).map_err(err)
}

#[tauri::command(async)]
fn test_rule(engine: E, rule: Rule, folder_id: Option<i64>) -> R<RuleTest> {
    engine.test_rule(&rule, folder_id).map_err(err)
}

#[tauri::command]
async fn run_rule(engine: E<'_>, id: i64, folder_id: Option<i64>, include_sending: bool) -> R<RuleRun> {
    engine.run_rule(id, folder_id, include_sending).await.map_err(err)
}

#[tauri::command(async)]
fn rule_facts(engine: E, ids: Vec<i64>) -> R<Vec<mail_core::rules::Facts>> {
    engine.rule_facts(&ids).map_err(err)
}

#[tauri::command(async)]
fn rule_activity(engine: E, limit: usize) -> R<Vec<RuleActivity>> {
    engine.rule_activity(limit).map_err(err)
}

#[tauri::command(async)]
fn undo_rule_activity(engine: E, id: i64) -> R<()> {
    engine.undo_rule_activity(id).map_err(err)
}

/// Atomically creates a new file in `dir` without overwriting existing files or following
/// dangling symlinks (`O_CREAT | O_EXCL`), picking a numbered suffix on collisions.
fn write_unique_file(dir: &Path, name: &str, bytes: &[u8]) -> R<PathBuf> {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) => (s.to_string(), format!(".{e}")),
        None => (name.to_string(), String::new()),
    };
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    for i in 0u32..10_000 {
        let candidate = if i == 0 { dir.join(name) } else { dir.join(format!("{stem} ({i}){ext}")) };
        match opts.open(&candidate) {
            Ok(mut f) => {
                f.write_all(bytes).map_err(|e| e.to_string())?;
                return Ok(candidate);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("Too many files with that name".into())
}

/// Extensions that can execute code or launch commands directly when opened via LaunchServices.
fn is_dangerous_extension(name: &str) -> bool {
    let ext = Path::new(name).extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase());
    matches!(
        ext.as_deref(),
        Some(
            "app" | "command" | "sh" | "bash" | "zsh" | "csh" | "scpt" | "scptd" | "applescript"
                | "terminal" | "workflow" | "action" | "inetloc" | "webloc" | "fileloc" | "pkg"
                | "mpkg" | "dmg" | "iso" | "jar" | "jnlp" | "exe" | "msi" | "bat" | "cmd" | "ps1"
                | "vbs" | "js" | "mjs" | "html" | "htm" | "svg" | "xhtml"
        )
    )
}

/// Writes the attachment to a temp folder and opens it with the default app.
#[tauri::command]
async fn open_attachment(app: tauri::AppHandle, engine: E<'_>, id: i64, index: usize) -> R<()> {
    let (name, bytes) = engine.attachment(id, index).await.map_err(err)?;
    let path = tokio::task::spawn_blocking(move || -> R<PathBuf> {
        let safe = safe_filename(&name);
        if is_dangerous_extension(&safe) {
            return Err(format!("“{safe}” can run code or scripts, so Mailvern won't open it directly. Save it to Downloads first if you trust the sender."));
        }
        let dir = std::env::temp_dir().join("Mailvern").join(format!("{id}-{index}"));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = write_unique_file(&dir, &safe, &bytes)?;
        quarantine_file(&path);
        Ok(path)
    })
    .await
    .map_err(|_| "Opening attachment failed".to_string())??;
    app.opener().open_path(path.to_string_lossy(), None::<&str>).map_err(|e| e.to_string())
}

/// Saves the attachment to Downloads and reveals it in Finder.
#[tauri::command]
async fn save_attachment(app: tauri::AppHandle, engine: E<'_>, id: i64, index: usize) -> R<String> {
    let (name, bytes) = engine.attachment(id, index).await.map_err(err)?;
    let path = tokio::task::spawn_blocking(move || -> R<PathBuf> {
        let dir = dirs::download_dir().ok_or("No Downloads folder")?;
        let path = write_unique_file(&dir, &safe_filename(&name), &bytes)?;
        quarantine_file(&path);
        Ok(path)
    })
    .await
    .map_err(|_| "Saving attachment failed".to_string())??;
    let _ = app.opener().reveal_item_in_dir(&path);
    Ok(path.to_string_lossy().into())
}

/// Saves every listed attachment of a message to `dest_dir` (or Downloads) and reveals the folder.
#[tauri::command]
async fn save_all_attachments(app: tauri::AppHandle, engine: E<'_>, id: i64, dest_dir: Option<String>) -> R<usize> {
    let files = engine.all_attachments(id).await.map_err(err)?;
    if files.is_empty() {
        return Ok(0);
    }
    let count = files.len();
    let last_path = tokio::task::spawn_blocking(move || -> R<Option<PathBuf>> {
        let dir = match dest_dir.filter(|s| !s.trim().is_empty()) {
            Some(d) => PathBuf::from(d),
            None => dirs::download_dir().ok_or("No Downloads folder")?,
        };
        let mut last = None;
        for (name, bytes) in &files {
            let path = write_unique_file(&dir, &safe_filename(name), bytes)?;
            quarantine_file(&path);
            last = Some(path);
        }
        Ok(last)
    })
    .await
    .map_err(|_| "Saving attachments failed".to_string())??;
    if let Some(p) = last_path {
        let _ = app.opener().reveal_item_in_dir(&p);
    }
    Ok(count)
}

/// Previews an attachment with macOS Quick Look (`qlmanage -p`).
#[tauri::command]
async fn quicklook_attachment(engine: E<'_>, id: i64, index: usize) -> R<()> {
    let (name, bytes) = engine.attachment(id, index).await.map_err(err)?;
    let path = tokio::task::spawn_blocking(move || -> R<PathBuf> {
        let safe = safe_filename(&name);
        let dir = std::env::temp_dir().join("Mailvern").join(format!("ql-{id}-{index}"));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = write_unique_file(&dir, &safe, &bytes)?;
        quarantine_file(&path);
        Ok(path)
    })
    .await
    .map_err(|_| "Preparing Quick Look failed".to_string())??;
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("/usr/bin/qlmanage")
            .arg("-p")
            .arg(&path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("Couldn't open Quick Look: {e}"))?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        Err("Quick Look is only available on macOS".into())
    }
}

/// Stages a single attachment in a temp directory so it can be dragged out or attached elsewhere.
#[tauri::command]
async fn stage_attachment(engine: E<'_>, id: i64, index: usize) -> R<FileInfo> {
    let (name, bytes) = engine.attachment(id, index).await.map_err(err)?;
    tokio::task::spawn_blocking(move || -> R<FileInfo> {
        let safe = safe_filename(&name);
        let dir = std::env::temp_dir().join("Mailvern").join(format!("drag-{id}-{index}"));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let candidate = dir.join(&safe);
        let path = if candidate.is_file() {
            candidate
        } else {
            let p = write_unique_file(&dir, &safe, &bytes)?;
            quarantine_file(&p);
            p
        };
        Ok(FileInfo { path: path.to_string_lossy().into(), name: safe, size: bytes.len() as u64 })
    })
    .await
    .map_err(|_| "Staging attachment failed".to_string())?
}

/// Writes a message's raw RFC 822 source to a temp `.eml` file so it can be attached to a draft.
#[tauri::command]
async fn stage_eml(engine: E<'_>, id: i64) -> R<FileInfo> {
    let (name, raw) = engine.raw_message(id).await.map_err(err)?;
    tokio::task::spawn_blocking(move || -> R<FileInfo> {
        let safe = safe_filename(&name);
        let dir = std::env::temp_dir().join("Mailvern").join(format!("eml-{id}"));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = write_unique_file(&dir, &safe, &raw)?;
        Ok(FileInfo { path: path.to_string_lossy().into(), name: safe, size: raw.len() as u64 })
    })
    .await
    .map_err(|_| "Staging message failed".to_string())?
}

/// Puts text on the clipboard natively without pulling in `tauri-plugin-clipboard-manager`
/// (`arboard` + `image` + `moxcms`).
#[cfg(target_os = "macos")]
#[tauri::command]
fn copy_text(text: String) -> R<()> {
    use objc2::msg_send;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject, Bool};
    use objc2_foundation::NSString;

    unsafe {
        let cls = AnyClass::get(c"NSPasteboard").ok_or("Clipboard unavailable")?;
        let pb: Option<Retained<AnyObject>> = msg_send![cls, generalPasteboard];
        let pb = pb.ok_or("Clipboard unavailable")?;
        let _: isize = msg_send![&*pb, clearContents];
        let ns_text = NSString::from_str(&text);
        let ns_type = NSString::from_str("public.utf8-plain-text");
        let ok: Bool = msg_send![&*pb, setString: &*ns_text, forType: &*ns_type];
        if ok.as_bool() {
            Ok(())
        } else {
            Err("Couldn't write to clipboard".into())
        }
    }
}

#[cfg(not(target_os = "macos"))]
#[tauri::command]
fn copy_text(_text: String) -> R<()> {
    Err("Clipboard write not supported on this platform".into())
}

#[derive(serde::Serialize)]
struct FileInfo {
    path: String,
    name: String,
    size: u64,
}

#[tauri::command(async)]
fn file_info(paths: Vec<String>) -> Vec<FileInfo> {
    paths
        .into_iter()
        .filter_map(|p| {
            let meta = std::fs::metadata(&p).ok()?;
            if !meta.is_file() {
                return None;
            }
            let name = PathBuf::from(&p).file_name()?.to_string_lossy().to_string();
            Some(FileInfo { path: p, name, size: meta.len() })
        })
        .collect()
}

/// Writes a message's attachments to a temp folder so they can be forwarded.
#[tauri::command]
async fn attachment_files(engine: E<'_>, id: i64) -> R<Vec<FileInfo>> {
    let files = engine.all_attachments(id).await.map_err(err)?;
    tokio::task::spawn_blocking(move || -> R<Vec<FileInfo>> {
        let dir = std::env::temp_dir().join("Mailvern").join(format!("fwd-{id}"));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for (name, bytes) in files {
            let safe = safe_filename(&name);
            let path = write_unique_file(&dir, &safe, &bytes)?;
            out.push(FileInfo { path: path.to_string_lossy().into(), name: safe, size: bytes.len() as u64 });
        }
        Ok(out)
    })
    .await
    .map_err(|_| "Preparing attachments failed".to_string())?
}

/// Reads an image file as a data: URL for inserting into the editor.
#[tauri::command(async)]
fn read_image(path: String) -> R<String> {
    use base64::Engine as _;
    const MAX: u64 = 15 * 1024 * 1024;
    let mime = match Path::new(&path).extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("heic") => "image/heic",
        _ => return Err("Not a supported image".into()),
    };
    let file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("Not a regular file".into());
    }
    if meta.len() > MAX {
        return Err("Image is larger than 15 MB".into());
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.take(MAX + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX {
        return Err("Image is larger than 15 MB".into());
    }
    Ok(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}

#[derive(serde::Serialize)]
struct SenderIcon {
    /// A `mailvern-icon://localhost/{domain}` URL served directly as binary bytes.
    url: String,
    /// A BIMI logo, made to fill the avatar. Otherwise a website icon.
    logo: bool,
}

/// The sender's domain logo or website icon, None to show initials.
#[tauri::command]
async fn sender_icon(engine: E<'_>, email: String) -> R<Option<SenderIcon>> {
    let Some(domain) = mail_core::icons::icon_domain(&email) else {
        return Ok(None);
    };
    Ok(engine.sender_icon(&email).await.map_err(err)?.map(|i| SenderIcon {
        url: format!("mailvern-icon://localhost/{domain}"),
        logo: i.logo,
    }))
}

fn parse_cid_uri(uri: &tauri::http::Uri) -> Option<(i64, usize)> {
    if uri.host() != Some("localhost") {
        return None;
    }
    let path = uri.path().strip_prefix('/')?;
    let (msg, part) = path.split_once('/')?;
    if part.contains('/') {
        return None;
    }
    let msg_id = msg.parse::<i64>().ok()?;
    let part_idx = part.parse::<usize>().ok()?;
    (msg_id > 0).then_some((msg_id, part_idx))
}

fn parse_icon_uri_domain(uri: &tauri::http::Uri) -> Option<String> {
    if uri.host() != Some("localhost") {
        return None;
    }
    let seg = uri.path().strip_prefix('/')?;
    if seg.is_empty() || seg.contains('/') {
        return None;
    }
    mail_core::icons::icon_domain(&format!("a@{seg}"))
}

fn empty_response(status: tauri::http::StatusCode) -> tauri::http::Response<Vec<u8>> {
    tauri::http::Response::builder()
        .status(status)
        .header(tauri::http::header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(Vec::new())
        .expect("static empty response")
}

#[tauri::command(async)]
fn contacts(engine: E, query: String) -> R<Vec<mail_core::model::Addr>> {
    engine.contacts(&query).map_err(err)
}

#[tauri::command(async)]
fn sent_recipients(engine: E) -> R<Vec<String>> {
    engine.sent_recipients().map_err(err)
}

#[tauri::command]
fn open_url(app: tauri::AppHandle, url: String) -> R<()> {
    let parsed = tauri::Url::parse(url.trim()).map_err(|_| "Unsupported link".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https" | "mailto") {
        return Err("Unsupported link".into());
    }
    app.opener().open_url(parsed.as_str(), None::<&str>).map_err(|e| e.to_string())
}

fn is_allowed_about_url(url: &tauri::Url) -> bool {
    url.scheme() == "about" && matches!(url.path(), "blank" | "srcdoc") && url.query().is_none()
}

/// The app's own pages, which the webview may navigate between.
fn is_app_url(app: &tauri::AppHandle, url: &tauri::Url) -> bool {
    match url.scheme() {
        "tauri" | "asset" | "ipc" => true,
        "about" => is_allowed_about_url(url),
        "http" | "https" => {
            (url.host_str() == Some("tauri.localhost") && url.port().is_none())
                || (tauri::is_dev() && app.config().build.dev_url.as_ref().is_some_and(|d| d.origin() == url.origin()))
        }
        _ => false,
    }
}

/// Links in a message would otherwise load inside its sandboxed frame (blank, since most sites
/// refuse to be framed). Any navigation away from the app is cancelled and opened in the browser.
/// Note: WebKit checks the parent document's `frame-src` CSP before firing `on_navigation` for
/// iframe clicks, so `tauri.conf.json` must allow `https: http: mailto:` in `frame-src`; this
/// handler cancels the navigation before anything loads in the frame.
/// On macOS it also trims WebKit's right-click menu in each webview (see `context_menu`).
fn external_links() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    tauri::plugin::Builder::new("external-links")
        .on_navigation(|webview, url| {
            let app = webview.app_handle();
            if is_app_url(app, url) {
                return true;
            }
            if url.scheme() == "mailto" {
                let _ = app.emit("open-mailto", url.as_str().to_string());
                return false;
            }
            if matches!(url.scheme(), "http" | "https") {
                let _ = app.opener().open_url(url.as_str(), None::<&str>);
            }
            false
        })
        .on_webview_ready(|webview| {
            #[cfg(target_os = "macos")]
            let _ = webview.with_webview(|w| context_menu::install(w.inner()));
            #[cfg(not(target_os = "macos"))]
            let _ = webview;
        })
        .build()
}

/// Removes stale extracted attachments and forward temp directories older than 24 hours.
fn cleanup_temp_dir() {
    let root = std::env::temp_dir().join("Mailvern");
    let Ok(entries) = std::fs::read_dir(&root) else { return };
    let max_age = std::time::Duration::from_secs(24 * 3600);
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age > max_age);
        if stale {
            if path.is_dir() {
                let _ = std::fs::remove_dir_all(&path);
            } else {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn default_app_data_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("app.mailvern"))
}

#[cfg(not(target_os = "macos"))]
fn default_app_data_dir() -> Option<PathBuf> {
    None
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    if let Ok(exe) = std::env::current_exe() {
        mail_core::extract::enable_subprocess_worker(exe);
    }

    // Start opening SQLite + precomputing the initial inbox snapshot BEFORE AppKit / WKWebView
    // initialize so cold SSD reads and query time overlap completely with WebKit XPC process boot.
    let early_preboot = default_app_data_dir().map(|dir| {
        let (eng_tx, eng_rx) = std::sync::mpsc::sync_channel::<anyhow::Result<Arc<Engine>>>(1);
        let (boot_tx, boot_rx) = std::sync::mpsc::sync_channel::<R<BootstrapData>>(1);
        let dir_bg = dir.clone();
        std::thread::spawn(move || {
            let opened = std::fs::create_dir_all(&dir_bg)
                .map_err(anyhow::Error::from)
                .and_then(|_| Engine::open(&dir_bg.join("mailvern.db")).map(Arc::new));
            match opened {
                Ok(engine) => {
                    let _ = eng_tx.send(Ok(engine.clone()));
                    let boot = engine
                        .bootstrap(&View::Unified { role: "inbox".into() }, BOOT_PAGE_LIMIT, STARTUP_SETTING_KEYS)
                        .map_err(err);
                    let _ = boot_tx.send(boot);
                }
                Err(e) => {
                    let _ = eng_tx.send(Err(e));
                }
            }
        });
        (dir, eng_rx, boot_rx)
    });

    tauri::Builder::default()
        .register_asynchronous_uri_scheme_protocol("mailvern-cid", |ctx, request, responder| {
            if request.method() != tauri::http::Method::GET {
                responder.respond(empty_response(tauri::http::StatusCode::METHOD_NOT_ALLOWED));
                return;
            }
            let Some((msg_id, part_idx)) = parse_cid_uri(request.uri()) else {
                responder.respond(empty_response(tauri::http::StatusCode::BAD_REQUEST));
                return;
            };
            let Some(engine) = ctx.app_handle().try_state::<Arc<Engine>>().map(|s| s.inner().clone()) else {
                responder.respond(empty_response(tauri::http::StatusCode::SERVICE_UNAVAILABLE));
                return;
            };
            tauri::async_runtime::spawn_blocking(move || {
                let resp = match engine.cid_image(msg_id, part_idx) {
                    Some((mime, bytes)) => tauri::http::Response::builder()
                        .status(tauri::http::StatusCode::OK)
                        .header(tauri::http::header::CONTENT_TYPE, mime)
                        .header(tauri::http::header::X_CONTENT_TYPE_OPTIONS, "nosniff")
                        .header(tauri::http::header::CACHE_CONTROL, "private, max-age=3600")
                        .body(bytes)
                        .unwrap_or_else(|_| empty_response(tauri::http::StatusCode::INTERNAL_SERVER_ERROR)),
                    None => empty_response(tauri::http::StatusCode::NOT_FOUND),
                };
                responder.respond(resp);
            });
        })
        .register_asynchronous_uri_scheme_protocol("mailvern-icon", |ctx, request, responder| {
            if request.method() != tauri::http::Method::GET {
                responder.respond(empty_response(tauri::http::StatusCode::METHOD_NOT_ALLOWED));
                return;
            }
            let Some(domain) = parse_icon_uri_domain(request.uri()) else {
                responder.respond(empty_response(tauri::http::StatusCode::BAD_REQUEST));
                return;
            };
            let Some(engine) = ctx.app_handle().try_state::<Arc<Engine>>().map(|s| s.inner().clone()) else {
                responder.respond(empty_response(tauri::http::StatusCode::SERVICE_UNAVAILABLE));
                return;
            };
            tauri::async_runtime::spawn_blocking(move || {
                let resp = match engine.cached_sender_icon(&domain) {
                    Ok(Some(icon)) => tauri::http::Response::builder()
                        .status(tauri::http::StatusCode::OK)
                        .header(tauri::http::header::CONTENT_TYPE, icon.mime)
                        .header(tauri::http::header::X_CONTENT_TYPE_OPTIONS, "nosniff")
                        .header(tauri::http::header::CONTENT_SECURITY_POLICY, "default-src 'none'; sandbox")
                        .header(tauri::http::header::CACHE_CONTROL, "private, max-age=86400")
                        .body(icon.data)
                        .unwrap_or_else(|_| empty_response(tauri::http::StatusCode::INTERNAL_SERVER_ERROR)),
                    _ => empty_response(tauri::http::StatusCode::NOT_FOUND),
                };
                responder.respond(resp);
            });
        })
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(external_links())
        // Reopen the window at the size and place it had when the app quit.
        // Visibility is left out: quitting while the window is hidden must not reopen it hidden.
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags({
                    use tauri_plugin_window_state::StateFlags;
                    StateFlags::SIZE | StateFlags::POSITION | StateFlags::MAXIMIZED | StateFlags::FULLSCREEN
                })
                .build(),
        )
        .setup(move |app| {
            let dir = app.path().app_data_dir()?;
            let (engine_res, preboot) = match early_preboot {
                Some((pre_dir, eng_rx, boot_rx)) if pre_dir == dir => {
                    let eng = eng_rx
                        .recv()
                        .unwrap_or_else(|_| Err(anyhow::anyhow!("Database startup thread exited unexpectedly")));
                    (eng, PrebootBootstrap::new(boot_rx))
                }
                _ => {
                    std::fs::create_dir_all(&dir)?;
                    (Engine::open(&dir.join("mailvern.db")).map(Arc::new), PrebootBootstrap::empty())
                }
            };
            let engine = match engine_res {
                Ok(e) => e,
                Err(e) => {
                    use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
                    app.dialog()
                        .message(format!("{e:#}"))
                        .title("Mailvern can't open its data")
                        .kind(MessageDialogKind::Error)
                        .blocking_show();
                    std::process::exit(1);
                }
            };
            let start_gate = Arc::new(StartGate::new());
            app.manage(engine.clone());
            app.manage(preboot);
            app.manage(start_gate.clone());
            app.manage(agents::Agents::default());
            // Apply the saved appearance and native window background before the page paints,
            // eliminating the bright-white flash in dark mode while WebKit finishes loading.
            let saved_theme = engine.setting("theme").ok().flatten();
            match saved_theme.as_deref() {
                Some("light") => app.set_theme(Some(tauri::Theme::Light)),
                Some("dark") => app.set_theme(Some(tauri::Theme::Dark)),
                _ => {}
            }
            if let Some(w) = app.get_webview_window("main") {
                let dark = match saved_theme.as_deref() {
                    Some("dark") => true,
                    Some("light") => false,
                    _ => matches!(w.theme(), Ok(tauri::Theme::Dark)),
                };
                let bg = if dark {
                    tauri::window::Color(27, 27, 26, 255)
                } else {
                    tauri::window::Color(255, 255, 255, 255)
                };
                let _ = w.set_background_color(Some(bg));
            }

            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let mut rx = engine.subscribe();
                // Wait until the first bootstrap snapshot is delivered (or 800ms fallback), plus a
                // short settle window so WebKit finishes frame-1 layout/paint without Keychain,
                // TLS, SQLite, or IPC contention from all accounts starting at t=0.
                let _ = tokio::time::timeout(std::time::Duration::from_millis(800), start_gate.wait()).await;
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                tokio::task::spawn_blocking(cleanup_temp_dir);
                if let Err(e) = engine.start() {
                    eprintln!("start failed: {e:#}");
                }
                handle.state::<agents::Agents>().apply(&engine).await;
                loop {
                    match rx.recv().await {
                        Ok(ev) => {
                            let _ = handle.emit("mail-event", ev);
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            // Catch up folders, list, thread, badge, and outbox after dropped events.
                            let _ = handle.emit("mail-event", mail_core::model::Event::Changed { account_id: 0, folder_id: None });
                            let _ = handle.emit("mail-event", mail_core::model::Event::Outbox);
                        }
                        Err(_) => break,
                    }
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            bootstrap,
            get_settings,
            accounts,
            save_account,
            delete_account,
            reorder_accounts,
            set_account_alerts,
            reorder_folders,
            create_folder,
            rename_folder,
            delete_folder,
            empty_folder,
            permanent_delete,
            folders,
            smart_counts,
            messages,
            thread,
            search,
            search_server,
            query_mail,
            group_mail,
            summaries,
            attachment_content,
            ai_key,
            set_ai_key,
            ai_http::ai_fetch,
            ai_http::ai_fetch_abort,
            move_to,
            snooze,
            unsnooze,
            cancel_send,
            send_now,
            outbox,
            save_draft,
            delete_draft,
            load_older,
            badge_count,
            mark_badge_seen,
            message,
            rsvp_invite,
            open_ics,
            raw_source,
            export_eml,
            cached_message,
            set_seen,
            mark_read,
            set_flagged,
            archive,
            trash,
            junk,
            not_junk,
            send,
            sync_now,
            sync_view,
            sync_account,
            get_setting,
            set_setting,
            open_attachment,
            save_attachment,
            save_all_attachments,
            quicklook_attachment,
            stage_attachment,
            stage_eml,
            copy_text,
            file_info,
            attachment_files,
            contacts,
            sent_recipients,
            sender_icon,
            read_image,
            open_url,
            agents::agent_access,
            agents::set_agent_access,
            agents::reset_agent_token,
            agents::set_agent_mode,
            agents::set_agent_account_mode,
            agents::agent_actions,
            agents::pending_agent_actions_count,
            agents::approve_agent_action,
            agents::reject_agent_action,
            agents::approve_all_agent_actions,
            agents::reject_all_agent_actions,
            rules,
            save_rule,
            delete_rule,
            reorder_rules,
            set_rule_enabled,
            check_rule,
            test_rule,
            run_rule,
            rule_activity,
            undo_rule_activity,
            rule_facts,
        ])
        // Like other Mac mail apps: closing the window (⌘W, red button) only hides it,
        // so sync and notifications keep running. ⌘Q still quits.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    // Notify the frontend so it can drop hidden keep-alive iframes while hidden,
                    // and prune stale temp files in the background during long sessions.
                    let _ = window.emit("window-hidden", ());
                    let engine = window.state::<Arc<Engine>>().inner().clone();
                    tauri::async_runtime::spawn_blocking(move || {
                        cleanup_temp_dir();
                        engine.maintenance();
                    });
                    // On macOS hide the app rather than the window: ⌘Tab only activates
                    // an app, it doesn't send Reopen, so a hidden window would stay gone.
                    // A hidden app comes back on ⌘Tab and Dock clicks alike.
                    #[cfg(target_os = "macos")]
                    let _ = window.app_handle().hide();
                    #[cfg(not(target_os = "macos"))]
                    let _ = window.hide();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building Mailvern")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                if let Some(engine) = app.try_state::<Arc<Engine>>() {
                    engine.maintenance();
                }
            }
            #[cfg(target_os = "macos")]
            match event {
                // Clicking the Dock icon brings the hidden window back.
                tauri::RunEvent::Reopen { .. } => show_main(app),
                // Default mail client: macOS delivers `mailto:` URLs via OpenURL AppleEvents.
                tauri::RunEvent::Opened { urls } => {
                    for url in urls {
                        if url.scheme() == "mailto" {
                            show_main(app);
                            let _ = app.emit("open-mailto", url.as_str().to_string());
                        }
                    }
                }
                _ => {}
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (app, event);
        });
}

fn show_main(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowed_about_urls_include_srcdoc_fragments() {
        assert!(is_allowed_about_url(&tauri::Url::parse("about:blank").unwrap()));
        assert!(is_allowed_about_url(&tauri::Url::parse("about:srcdoc").unwrap()));
        assert!(is_allowed_about_url(&tauri::Url::parse("about:srcdoc#section-1").unwrap()));
        assert!(!is_allowed_about_url(&tauri::Url::parse("about:srcdoc?foo=bar").unwrap()));
        assert!(!is_allowed_about_url(&tauri::Url::parse("about:settings").unwrap()));
    }

    #[test]
    fn csp_frame_src_allows_external_email_links_to_reach_navigation_hook() {
        let conf: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let csp = conf["app"]["security"]["csp"].as_str().expect("security.csp must be set");
        let frame_src = csp
            .split(';')
            .map(str::trim)
            .find(|d| d.starts_with("frame-src "))
            .expect("CSP must define frame-src");
        let tokens: Vec<&str> = frame_src.split_whitespace().collect();
        for required in ["'self'", "about:", "https:", "http:", "mailto:"] {
            assert!(tokens.contains(&required), "frame-src must include {required}, got: {frame_src}");
        }
        let img_src = csp
            .split(';')
            .map(str::trim)
            .find(|d| d.starts_with("img-src "))
            .expect("CSP must define img-src");
        let img_tokens: Vec<&str> = img_src.split_whitespace().collect();
        for required in ["mailvern-cid:", "mailvern-icon:"] {
            assert!(img_tokens.contains(&required), "img-src must include {required}, got: {img_src}");
        }
    }

    #[test]
    fn custom_uri_scheme_parsers_reject_invalid_inputs() {
        assert_eq!(parse_cid_uri(&"mailvern-cid://localhost/42/3".parse().unwrap()), Some((42, 3)));
        assert_eq!(parse_cid_uri(&"mailvern-cid://evil/42/3".parse().unwrap()), None);
        assert_eq!(parse_cid_uri(&"mailvern-cid://localhost/0/3".parse().unwrap()), None);
        assert_eq!(parse_cid_uri(&"mailvern-cid://localhost/42/3/extra".parse().unwrap()), None);
        assert_eq!(parse_cid_uri(&"mailvern-cid://localhost/../3".parse().unwrap()), None);

        assert_eq!(parse_icon_uri_domain(&"mailvern-icon://localhost/stripe.com".parse().unwrap()).as_deref(), Some("stripe.com"));
        assert_eq!(parse_icon_uri_domain(&"mailvern-icon://evil/stripe.com".parse().unwrap()), None);
        assert_eq!(parse_icon_uri_domain(&"mailvern-icon://localhost/gmail.com".parse().unwrap()), None);
        assert_eq!(parse_icon_uri_domain(&"mailvern-icon://localhost/stripe.com/extra".parse().unwrap()), None);
    }
}
