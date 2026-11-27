use super::{err, E, R};
use mail_core::model::{Account, DiscoveredServers, Folder, SavedSearch, SearchScope, SmartCounts, View};

#[tauri::command(async)]
pub fn accounts(engine: E) -> R<Vec<Account>> {
    engine.accounts().map_err(err)
}

#[tauri::command]
pub async fn save_account(engine: E<'_>, account: Account, password: Option<String>) -> R<Account> {
    engine.save_account(account, password).await.map_err(err)
}

#[tauri::command]
pub async fn discover_servers(engine: E<'_>, email: String) -> R<Option<DiscoveredServers>> {
    Ok(engine.discover_servers(&email).await)
}

#[tauri::command(async)]
pub fn saved_searches(engine: E) -> R<Vec<SavedSearch>> {
    engine.saved_searches().map_err(err)
}

#[tauri::command(async)]
pub fn save_saved_search(engine: E, id: i64, name: String, query: String, scope: SearchScope) -> R<SavedSearch> {
    engine.save_saved_search(id, &name, &query, &scope).map_err(err)
}

#[tauri::command(async)]
pub fn delete_saved_search(engine: E, id: i64) -> R<()> {
    engine.delete_saved_search(id).map_err(err)
}

#[tauri::command(async)]
pub fn reorder_saved_searches(engine: E, ids: Vec<i64>) -> R<()> {
    engine.reorder_saved_searches(&ids).map_err(err)
}

#[tauri::command(async)]
pub fn delete_account(engine: E, id: i64) -> R<()> {
    engine.delete_account(id).map_err(err)
}

#[tauri::command(async)]
pub fn reorder_accounts(engine: E, ids: Vec<i64>) -> R<()> {
    engine.reorder_accounts(&ids).map_err(err)
}

#[tauri::command(async)]
pub fn set_account_alerts(engine: E, account_id: i64, notify: bool, badge_mode: String) -> R<()> {
    engine.set_account_alerts(account_id, notify, &badge_mode).map_err(err)
}

#[tauri::command(async)]
pub fn reorder_folders(engine: E, account_id: i64, ids: Vec<i64>) -> R<()> {
    engine.reorder_folders(account_id, &ids).map_err(err)
}

#[tauri::command]
pub async fn create_folder(engine: E<'_>, account_id: i64, name: String, parent_id: Option<i64>) -> R<Folder> {
    engine.create_folder(account_id, &name, parent_id).await.map_err(err)
}

#[tauri::command]
pub async fn rename_folder(engine: E<'_>, folder_id: i64, new_name: String) -> R<()> {
    engine.rename_folder(folder_id, &new_name).await.map_err(err)
}

#[tauri::command]
pub async fn delete_folder(engine: E<'_>, folder_id: i64) -> R<()> {
    engine.delete_folder(folder_id).await.map_err(err)
}

#[tauri::command(async)]
pub fn empty_folder(engine: E, view: View) -> R<()> {
    engine.empty_folder(&view).map_err(err)
}


#[tauri::command(async)]
pub fn folders(engine: E) -> R<Vec<Folder>> {
    engine.folders().map_err(err)
}

#[tauri::command(async)]
pub fn smart_counts(engine: E) -> R<SmartCounts> {
    engine.smart_counts().map_err(err)
}

