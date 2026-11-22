use mail_core::model::{Account, Folder, MessageDetail, MessageSummary, Outgoing, View};
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
fn folders(engine: E) -> R<Vec<Folder>> {
    engine.folders().map_err(err)
}

#[tauri::command]
fn messages(engine: E, view: View, query: Option<String>, offset: i64, limit: i64) -> R<Vec<MessageSummary>> {
    engine.messages(&view, query.as_deref(), offset, limit).map_err(err)
}

#[tauri::command]
fn search(engine: E, query: String) -> R<Vec<MessageSummary>> {
    engine.search_all(&query).map_err(err)
}

#[tauri::command]
fn unread_count(engine: E) -> R<i64> {
    engine.unread_count().map_err(err)
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
async fn send(engine: E<'_>, message: Outgoing) -> R<()> {
    engine.send(message).await.map_err(err)
}

#[tauri::command]
fn sync_now(engine: E) {
    engine.sync_now();
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let engine = Arc::new(Engine::open(&dir.join("mailvern.db"))?);
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
            folders,
            messages,
            search,
            unread_count,
            message,
            set_seen,
            set_flagged,
            archive,
            trash,
            send,
            sync_now,
            get_setting,
            set_setting,
            open_attachment,
            save_attachment,
            file_info,
            attachment_files,
            contacts,
            open_url,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Mailvern");
}
