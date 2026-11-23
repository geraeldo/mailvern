use mail_core::model::{
    Account, AttachmentContent, Folder, MailGroup, MailQuery, MessageDetail, MessageSummary, Outgoing, OutboxItem, SearchScope, View,
};
use mail_core::Engine;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{Emitter, Manager, State};
use tauri_plugin_opener::OpenerExt;

type E<'a> = State<'a, Arc<Engine>>;
type R<T> = Result<T, String>;

fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

#[tauri::command]
fn accounts(engine: E) -> R<Vec<Account>> {
    engine.accounts().map_err(err)
}

#[tauri::command]
async fn save_account(engine: E<'_>, account: Account, password: Option<String>) -> R<Account> {
    engine.save_account(account, password).await.map_err(err)
}

#[tauri::command]
fn delete_account(engine: E, id: i64) -> R<()> {
    engine.delete_account(id).map_err(err)
}

#[tauri::command]
fn reorder_accounts(engine: E, ids: Vec<i64>) -> R<()> {
    engine.reorder_accounts(&ids).map_err(err)
}

#[tauri::command]
fn reorder_folders(engine: E, account_id: i64, ids: Vec<i64>) -> R<()> {
    engine.reorder_folders(account_id, &ids).map_err(err)
}

#[tauri::command]
fn folders(engine: E) -> R<Vec<Folder>> {
    engine.folders().map_err(err)
}

#[tauri::command]
fn messages(engine: E, view: View, query: Option<String>, offset: i64, limit: i64, threaded: Option<bool>) -> R<Vec<MessageSummary>> {
    engine.messages(&view, query.as_deref(), offset, limit, threaded.unwrap_or(true)).map_err(err)
}

#[tauri::command]
fn thread(engine: E, id: i64) -> R<Vec<MessageSummary>> {
    engine.thread(id).map_err(err)
}

#[tauri::command]
async fn search_server(engine: E<'_>, query: String, scope: Option<SearchScope>) -> R<Vec<MessageSummary>> {
    engine.search_server(&query, &scope.unwrap_or_default()).await.map_err(err)
}

#[tauri::command]
fn move_to(engine: E, ids: Vec<i64>, folder_id: i64) -> R<()> {
    engine.move_to(&ids, folder_id).map_err(err)
}

#[tauri::command]
fn snooze(engine: E, ids: Vec<i64>, until: i64) -> R<()> {
    engine.snooze(&ids, until).map_err(err)
}

#[tauri::command]
fn unsnooze(engine: E, ids: Vec<i64>) -> R<()> {
    engine.unsnooze(&ids).map_err(err)
}

#[tauri::command]
async fn load_older(engine: E<'_>, view: View) -> R<usize> {
    engine.load_older(&view).await.map_err(err)
}

#[tauri::command]
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

#[tauri::command]
fn group_mail(engine: E, query: MailQuery, by: String) -> R<GroupResult> {
    let (total, groups) = engine.group_mail(&query, &by).map_err(err)?;
    Ok(GroupResult { total, groups })
}

#[tauri::command]
fn summaries(engine: E, ids: Vec<i64>) -> R<Vec<MessageSummary>> {
    engine.summaries(&ids).map_err(err)
}

#[tauri::command]
async fn attachment_content(engine: E<'_>, id: i64, index: usize, offset: Option<usize>, max_chars: Option<usize>) -> R<AttachmentContent> {
    let max = max_chars.unwrap_or(20_000).clamp(500, 200_000);
    engine.attachment_content(id, index, offset.unwrap_or(0), max).await.map_err(err)
}

/// The OpenRouter API key lives in the Keychain, like account passwords.
const AI_KEY: &str = "openrouter-api-key";

#[tauri::command]
fn ai_key() -> Option<String> {
    mail_core::secrets::get(AI_KEY).ok().filter(|k| !k.is_empty())
}

/// Stores the key, or removes it when `key` is empty.
#[tauri::command]
fn set_ai_key(key: String) -> R<()> {
    let key = key.trim();
    if key.is_empty() {
        mail_core::secrets::delete(AI_KEY);
        Ok(())
    } else {
        mail_core::secrets::set(AI_KEY, key).map_err(err)
    }
}

#[tauri::command]
fn badge_count(engine: E) -> R<i64> {
    engine.badge_count().map_err(err)
}

#[tauri::command]
fn mark_badge_seen(engine: E) -> R<()> {
    engine.mark_badge_seen().map_err(err)
}

#[tauri::command]
async fn message(engine: E<'_>, id: i64) -> R<MessageDetail> {
    engine.message(id).await.map_err(err)
}

#[tauri::command]
fn set_seen(engine: E, ids: Vec<i64>, seen: bool) -> R<()> {
    engine.set_seen(&ids, seen).map_err(err)
}

#[tauri::command]
fn mark_read(engine: E, view: View) -> R<()> {
    engine.mark_read(&view).map_err(err)
}

#[tauri::command]
fn set_flagged(engine: E, ids: Vec<i64>, flagged: bool) -> R<()> {
    engine.set_flagged(&ids, flagged).map_err(err)
}

#[tauri::command]
fn archive(engine: E, ids: Vec<i64>) -> R<()> {
    engine.archive(&ids).map_err(err)
}

#[tauri::command]
fn trash(engine: E, ids: Vec<i64>) -> R<()> {
    engine.trash(&ids).map_err(err)
}

#[tauri::command]
fn junk(engine: E, ids: Vec<i64>) -> R<()> {
    engine.junk(&ids).map_err(err)
}

#[tauri::command]
fn not_junk(engine: E, ids: Vec<i64>) -> R<()> {
    engine.not_junk(&ids).map_err(err)
}

/// Queues a message: after the undo window, or at `send_at` (unix seconds). Returns its outbox id.
#[tauri::command]
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
#[tauri::command]
fn cancel_send(engine: E, id: i64) -> R<Cancelled> {
    let (message, files) = engine.cancel_send(id).map_err(err)?;
    let dir = std::env::temp_dir().join("Mailvern").join(format!("outbox-{id}"));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut attachments = Vec::new();
    for (name, bytes) in files {
        let path = unique_path(&dir, &safe_name(&name));
        std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
        attachments.push(FileInfo { path: path.to_string_lossy().into(), name: safe_name(&name), size: bytes.len() as u64 });
    }
    Ok(Cancelled { message, attachments })
}

#[tauri::command]
fn send_now(engine: E, id: i64) -> R<()> {
    engine.send_now(id).map_err(err)
}

#[tauri::command]
fn outbox(engine: E) -> R<Vec<OutboxItem>> {
    engine.outbox().map_err(err)
}

#[tauri::command]
async fn save_draft(engine: E<'_>, message: Outgoing) -> R<String> {
    engine.save_draft(message).await.map_err(err)
}

#[tauri::command]
fn delete_draft(engine: E, account_id: i64, message_id: String) -> R<()> {
    engine.delete_draft(account_id, &message_id).map_err(err)
}

#[tauri::command]
fn sync_now(engine: E) {
    engine.sync_now();
}

#[tauri::command]
fn sync_account(engine: E, account_id: i64) -> R<()> {
    engine.sync_account(account_id).map_err(err)
}

#[tauri::command]
fn sync_view(engine: E, view: View) -> R<()> {
    engine.sync_view(&view).map_err(err)
}

#[tauri::command]
fn get_setting(engine: E, key: String) -> R<Option<String>> {
    engine.setting(&key).map_err(err)
}

#[tauri::command]
fn set_setting(engine: E, key: String, value: String) -> R<()> {
    engine.set_setting(&key, &value).map_err(err)
}

fn safe_name(name: &str) -> String {
    let n: String = name.chars().map(|c| if matches!(c, '/' | '\\' | ':' | '\0') { '_' } else { c }).collect();
    let n = n.trim_start_matches('.').trim().to_string();
    if n.is_empty() { "attachment".into() } else { n }
}

fn unique_path(dir: &PathBuf, name: &str) -> PathBuf {
    let p = dir.join(name);
    if !p.exists() {
        return p;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) => (s.to_string(), format!(".{e}")),
        None => (name.to_string(), String::new()),
    };
    (1..)
        .map(|i| dir.join(format!("{stem} ({i}){ext}")))
        .find(|p| !p.exists())
        .unwrap()
}

/// Writes the attachment to a temp folder and opens it with the default app.
#[tauri::command]
async fn open_attachment(app: tauri::AppHandle, engine: E<'_>, id: i64, index: usize) -> R<()> {
    let (name, bytes) = engine.attachment(id, index).await.map_err(err)?;
    let dir = std::env::temp_dir().join("Mailvern").join(format!("{id}-{index}"));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(safe_name(&name));
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    app.opener().open_path(path.to_string_lossy(), None::<&str>).map_err(|e| e.to_string())
}

/// Saves the attachment to Downloads and reveals it in Finder.
#[tauri::command]
async fn save_attachment(app: tauri::AppHandle, engine: E<'_>, id: i64, index: usize) -> R<String> {
    let (name, bytes) = engine.attachment(id, index).await.map_err(err)?;
    let dir = dirs::download_dir().ok_or("No Downloads folder")?;
    let path = unique_path(&dir, &safe_name(&name));
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    let _ = app.opener().reveal_item_in_dir(&path);
    Ok(path.to_string_lossy().into())
}

#[derive(serde::Serialize)]
struct FileInfo {
    path: String,
    name: String,
    size: u64,
}

#[tauri::command]
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
    let dir = std::env::temp_dir().join("Mailvern").join(format!("fwd-{id}"));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for (name, bytes) in files {
        let path = unique_path(&dir, &safe_name(&name));
        std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
        out.push(FileInfo { path: path.to_string_lossy().into(), name: safe_name(&name), size: bytes.len() as u64 });
    }
    Ok(out)
}

/// Reads an image file as a data: URL for inserting into the editor.
#[tauri::command]
fn read_image(path: String) -> R<String> {
    use base64::Engine as _;
    let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
    if meta.len() > 15 * 1024 * 1024 {
        return Err("Image is larger than 15 MB".into());
    }
    let mime = match PathBuf::from(&path).extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("heic") => "image/heic",
        _ => return Err("Not a supported image".into()),
    };
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    Ok(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}

#[tauri::command]
fn contacts(engine: E, query: String) -> R<Vec<mail_core::model::Addr>> {
    engine.contacts(&query).map_err(err)
}

#[tauri::command]
fn open_url(app: tauri::AppHandle, url: String) -> R<()> {
    if !(url.starts_with("http://") || url.starts_with("https://") || url.starts_with("mailto:")) {
        return Err("Unsupported link".into());
    }
    app.opener().open_url(url, None::<&str>).map_err(|e| e.to_string())
}

/// The app's own pages, which the webview may navigate between.
fn is_app_url(app: &tauri::AppHandle, url: &tauri::Url) -> bool {
    match url.scheme() {
        "tauri" | "about" | "data" | "blob" | "asset" | "ipc" => true,
        "http" | "https" => {
            url.host_str() == Some("tauri.localhost")
                || (tauri::is_dev() && app.config().build.dev_url.as_ref().is_some_and(|d| d.origin() == url.origin()))
        }
        _ => false,
    }
}

/// Links in a message would otherwise load inside its sandboxed frame (blank, since most sites
/// refuse to be framed). Any navigation away from the app is cancelled and opened in the browser.
fn external_links() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    tauri::plugin::Builder::new("external-links")
        .on_navigation(|webview, url| {
            let app = webview.app_handle();
            if is_app_url(app, url) {
                return true;
            }
            if matches!(url.scheme(), "http" | "https" | "mailto") {
                let _ = app.opener().open_url(url.as_str(), None::<&str>);
            }
            false
        })
        .build()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(external_links())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let engine = match Engine::open(&dir.join("mailvern.db")) {
                Ok(e) => Arc::new(e),
                Err(e) => {
                    use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
                    app.dialog()
                        .message(format!("{e:#}"))
                        .title("Mailvern can't open its data")
                        .kind(MessageDialogKind::Error)
                        .show(|_| std::process::exit(1));
                    return Ok(());
                }
            };
            app.manage(engine.clone());

            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = engine.start() {
                    eprintln!("start failed: {e:#}");
                }
                let mut rx = engine.subscribe();
                loop {
                    match rx.recv().await {
                        Ok(ev) => {
                            let _ = handle.emit("mail-event", ev);
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(_) => break,
                    }
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            accounts,
            save_account,
            delete_account,
            reorder_accounts,
            reorder_folders,
            folders,
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
            file_info,
            attachment_files,
            contacts,
            read_image,
            open_url,
        ])
        // Like other Mac mail apps: closing the window (⌘W, red button) only hides it,
        // so sync and notifications keep running. ⌘Q still quits.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building Mailvern")
        .run(|app, event| {
            // Clicking the Dock icon brings the hidden window back.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                show_main(app);
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
