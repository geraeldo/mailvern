use super::files::{write_unique_file, FileInfo};
use super::{err, E, R};
use mail_core::model::{MailGroup, MailQuery, MessageDetail, MessageSummary, OutboxItem, Outgoing, SearchScope, View};
use mail_core::text::{quarantine_file, safe_filename};
use std::path::PathBuf;
use tauri_plugin_opener::OpenerExt;

#[tauri::command(async)]
pub fn permanent_delete(engine: E, ids: Vec<i64>) -> R<()> {
    engine.permanent_delete(&ids).map_err(err)
}


#[tauri::command(async)]
pub fn messages(engine: E, view: View, query: Option<String>, offset: i64, limit: i64, threaded: Option<bool>) -> R<Vec<MessageSummary>> {
    engine.messages(&view, query.as_deref(), offset, limit, threaded.unwrap_or(true)).map_err(err)
}

#[tauri::command(async)]
pub fn thread(engine: E, id: i64) -> R<Vec<MessageSummary>> {
    engine.thread(id).map_err(err)
}

#[tauri::command]
pub async fn search_server(engine: E<'_>, query: String, scope: Option<SearchScope>) -> R<Vec<MessageSummary>> {
    engine.search_server(&query, &scope.unwrap_or_default()).await.map_err(err)
}

#[tauri::command(async)]
pub fn move_to(engine: E, ids: Vec<i64>, folder_id: i64) -> R<()> {
    engine.move_to(&ids, folder_id).map_err(err)
}

#[tauri::command(async)]
pub fn snooze(engine: E, ids: Vec<i64>, until: i64) -> R<()> {
    engine.snooze(&ids, until).map_err(err)
}

#[tauri::command(async)]
pub fn unsnooze(engine: E, ids: Vec<i64>) -> R<()> {
    engine.unsnooze(&ids).map_err(err)
}

#[tauri::command]
pub async fn load_older(engine: E<'_>, view: View) -> R<usize> {
    engine.load_older(&view).await.map_err(err)
}

#[tauri::command(async)]
pub fn search(engine: E, query: String, scope: Option<SearchScope>) -> R<Vec<MessageSummary>> {
    engine.search(&query, &scope.unwrap_or_default()).map_err(err)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryResult {
    total: i64,
    rows: Vec<MessageSummary>,
    /// Set when `server` was asked for and every server failed (local results still come back).
    server_error: Option<String>,
}

/// Structured search for the AI tools. With `server`, the mail servers are asked first.
#[tauri::command]
pub async fn query_mail(engine: E<'_>, query: MailQuery, server: Option<bool>) -> R<QueryResult> {
    let (r, server_error) = if server.unwrap_or(false) {
        engine.query_mail_server(&query).await.map_err(err)?
    } else {
        (engine.query_mail(&query).map_err(err)?, None)
    };
    Ok(QueryResult { total: r.total, rows: r.rows, server_error })
}

#[derive(serde::Serialize)]
pub struct GroupResult {
    total: i64,
    groups: Vec<MailGroup>,
}

#[tauri::command(async)]
pub fn group_mail(engine: E, query: MailQuery, by: String) -> R<GroupResult> {
    let (total, groups) = engine.group_mail(&query, &by).map_err(err)?;
    Ok(GroupResult { total, groups })
}

#[tauri::command(async)]
pub fn summaries(engine: E, ids: Vec<i64>) -> R<Vec<MessageSummary>> {
    engine.summaries(&ids).map_err(err)
}


#[tauri::command]
pub async fn message(engine: E<'_>, id: i64) -> R<MessageDetail> {
    engine.message(id).await.map_err(err)
}

#[tauri::command]
pub async fn rsvp_invite(engine: E<'_>, id: i64, status: String) -> R<String> {
    engine.rsvp_invite(id, &status).await.map_err(err)
}

#[tauri::command]
pub async fn open_ics(app: tauri::AppHandle, engine: E<'_>, id: i64) -> R<()> {
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
pub async fn raw_source(engine: E<'_>, id: i64) -> R<String> {
    let (_, raw) = engine.raw_message(id).await.map_err(err)?;
    tokio::task::spawn_blocking(move || String::from_utf8_lossy(&raw).into_owned())
        .await
        .map_err(|_| "Decoding message source failed".to_string())
}

#[tauri::command]
pub async fn export_eml(app: tauri::AppHandle, engine: E<'_>, id: i64, dest: Option<String>) -> R<String> {
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
pub fn cached_message(engine: E, id: i64) -> R<Option<MessageDetail>> {
    engine.cached_message(id).map_err(err)
}

#[tauri::command(async)]
pub fn set_seen(engine: E, ids: Vec<i64>, seen: bool) -> R<()> {
    engine.set_seen(&ids, seen).map_err(err)
}

#[tauri::command(async)]
pub fn mark_read(engine: E, view: View) -> R<()> {
    engine.mark_read(&view).map_err(err)
}

#[tauri::command(async)]
pub fn set_flagged(engine: E, ids: Vec<i64>, flagged: bool) -> R<()> {
    engine.set_flagged(&ids, flagged).map_err(err)
}

#[tauri::command(async)]
pub fn archive(engine: E, ids: Vec<i64>) -> R<()> {
    engine.archive(&ids).map_err(err)
}

#[tauri::command(async)]
pub fn trash(engine: E, ids: Vec<i64>) -> R<()> {
    engine.trash(&ids).map_err(err)
}

#[tauri::command(async)]
pub fn junk(engine: E, ids: Vec<i64>) -> R<()> {
    engine.junk(&ids).map_err(err)
}

#[tauri::command(async)]
pub fn not_junk(engine: E, ids: Vec<i64>) -> R<()> {
    engine.not_junk(&ids).map_err(err)
}

/// Queues a message: after the undo window, or at `send_at` (unix seconds). Returns its outbox id.
#[tauri::command(async)]
pub fn send(engine: E, message: Outgoing, send_at: Option<i64>) -> R<i64> {
    engine.send(message, send_at).map_err(err)
}

#[derive(serde::Serialize)]
pub struct Cancelled {
    message: Outgoing,
    attachments: Vec<FileInfo>,
}

/// Takes a message back out of the outbox to edit it. Its attachments are written to a
/// temp folder so the composer can attach them again.
#[tauri::command(async)]
pub fn cancel_send(engine: E, id: i64) -> R<Cancelled> {
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
pub fn send_now(engine: E, id: i64) -> R<()> {
    engine.send_now(id).map_err(err)
}

#[tauri::command(async)]
pub fn outbox(engine: E) -> R<Vec<OutboxItem>> {
    engine.outbox().map_err(err)
}

#[tauri::command]
pub async fn save_draft(engine: E<'_>, message: Outgoing) -> R<String> {
    engine.save_draft(message).await.map_err(err)
}

#[tauri::command(async)]
pub fn delete_draft(engine: E, account_id: i64, message_id: String) -> R<()> {
    engine.delete_draft(account_id, &message_id).map_err(err)
}

#[tauri::command(async)]
pub fn sync_now(engine: E) {
    engine.sync_now();
}

#[tauri::command(async)]
pub fn sync_account(engine: E, account_id: i64) -> R<()> {
    engine.sync_account(account_id).map_err(err)
}

#[tauri::command(async)]
pub fn sync_view(engine: E, view: View) -> R<()> {
    engine.sync_view(&view).map_err(err)
}

