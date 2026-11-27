//! Access for coding agents on this Mac (Claude Code, Codex, Opencode): runs the MCP server from
//! `crates/mail-mcp` while the setting is on. Off by default.

use mail_core::model::{AgentAction, AgentMode, BatchOutcome};
use mail_core::Engine;
use mail_mcp::{Server, DEFAULT_PORT};
use std::collections::HashMap;
use std::sync::Arc;
use tauri::State;

/// Settings key: "on" when agents may connect.
pub(crate) const ENABLED: &str = "mcp_enabled";
/// Settings key: global default MCP permission mode ("approval" | "auto").
pub(crate) const MODE: &str = mail_core::AGENT_MODE_KEY;
/// Legacy SQLite setting key for the token, migrated into macOS Keychain on first read.
pub(crate) const LEGACY_TOKEN: &str = "mcp_token";
/// macOS Keychain account name for the MCP bearer token.
const KEYCHAIN_TOKEN: &str = "mcp-access-token";

#[derive(Default)]
pub struct Agents {
    /// The running server, or why it couldn't start.
    state: tokio::sync::Mutex<(Option<Server>, Option<String>)>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentAccess {
    enabled: bool,
    running: bool,
    url: String,
    /// Shown in Settings so the setup command can be copied. Only while access is on.
    token: Option<String>,
    error: Option<String>,
    mode: AgentMode,
    account_modes: HashMap<i64, AgentMode>,
    disabled_accounts: Vec<i64>,
    folder_overrides: HashMap<i64, bool>,
}

fn setting(engine: &Engine, key: &str) -> Option<String> {
    engine.setting(key).ok().flatten().filter(|v| !v.is_empty())
}

fn valid_token(t: &str) -> bool {
    t.len() >= 16 && t.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

/// Loads the MCP bearer token from Keychain, migrating any legacy plaintext SQLite value.
fn load_token(engine: &Engine) -> Option<String> {
    if let Ok(t) = mail_core::secrets::get(KEYCHAIN_TOKEN) {
        if valid_token(&t) {
            let _ = engine.delete_setting(LEGACY_TOKEN);
            return Some(t);
        }
    }
    if let Some(legacy) = setting(engine, LEGACY_TOKEN).filter(|t| valid_token(t)) {
        if mail_core::secrets::set(KEYCHAIN_TOKEN, &legacy).is_ok() {
            let _ = engine.delete_setting(LEGACY_TOKEN);
        }
        return Some(legacy);
    }
    None
}

fn store_token(engine: &Engine, token: &str) -> Result<(), String> {
    mail_core::secrets::set(KEYCHAIN_TOKEN, token).map_err(|e| format!("{e:#}"))?;
    let _ = engine.delete_setting(LEGACY_TOKEN);
    Ok(())
}

impl Agents {
    /// Starts or stops the server to match the settings. The token is read at start,
    /// so this also puts a new token into effect.
    pub async fn apply(&self, engine: &Arc<Engine>) -> AgentAccess {
        let mut st = self.state.lock().await;
        self.apply_locked(&mut st, engine).await
    }

    async fn apply_locked(&self, st: &mut (Option<Server>, Option<String>), engine: &Arc<Engine>) -> AgentAccess {
        if let Some(old) = st.0.take() {
            old.stop().await;
        }
        st.1 = None;
        let enabled = setting(engine, ENABLED).as_deref() == Some("on");
        // Only touch macOS Keychain when Agent Access is actually enabled.
        let token = if enabled { load_token(engine) } else { None };
        if enabled {
            match &token {
                Some(t) => match Server::start(engine.clone(), DEFAULT_PORT, t.clone()).await {
                    Ok(server) => st.0 = Some(server),
                    Err(e) => st.1 = Some(format!("{e:#}. Another copy of Mailvern or another app may be using it.")),
                },
                None => st.1 = Some("Missing access token in Keychain. Reset the token to generate a new one.".into()),
            }
        }
        let perms = engine.agent_permissions().unwrap_or_default();
        AgentAccess {
            enabled,
            running: st.0.is_some(),
            url: mail_mcp::url(DEFAULT_PORT),
            token,
            error: st.1.clone(),
            mode: perms.default_mode,
            account_modes: perms.account_modes,
            disabled_accounts: perms.disabled_accounts,
            folder_overrides: perms.folder_overrides,
        }
    }

    async fn status(&self, engine: &Engine) -> AgentAccess {
        let st = self.state.lock().await;
        let enabled = setting(engine, ENABLED).as_deref() == Some("on");
        let perms = engine.agent_permissions().unwrap_or_default();
        AgentAccess {
            enabled,
            running: st.0.is_some(),
            url: mail_mcp::url(DEFAULT_PORT),
            token: if enabled { load_token(engine) } else { None },
            error: st.1.clone(),
            mode: perms.default_mode,
            account_modes: perms.account_modes,
            disabled_accounts: perms.disabled_accounts,
            folder_overrides: perms.folder_overrides,
        }
    }
}

type R = Result<AgentAccess, String>;

#[tauri::command]
pub async fn agent_access(engine: State<'_, Arc<Engine>>, agents: State<'_, Agents>) -> R {
    Ok(agents.status(&engine).await)
}

#[tauri::command]
pub async fn set_agent_access(engine: State<'_, Arc<Engine>>, agents: State<'_, Agents>, enabled: bool) -> R {
    let mut st = agents.state.lock().await;
    if enabled && load_token(&engine).is_none() {
        store_token(&engine, &mail_mcp::new_token())?;
    }
    engine.set_setting(ENABLED, if enabled { "on" } else { "off" }).map_err(|e| format!("{e:#}"))?;
    Ok(agents.apply_locked(&mut st, &engine).await)
}

/// Replaces the token, which cuts off every agent set up with the old one.
#[tauri::command]
pub async fn reset_agent_token(engine: State<'_, Arc<Engine>>, agents: State<'_, Agents>) -> R {
    let mut st = agents.state.lock().await;
    store_token(&engine, &mail_mcp::new_token())?;
    Ok(agents.apply_locked(&mut st, &engine).await)
}

#[tauri::command]
pub async fn set_agent_mode(engine: State<'_, Arc<Engine>>, agents: State<'_, Agents>, mode: AgentMode) -> R {
    engine.set_agent_default_mode(mode).map_err(|e| format!("{e:#}"))?;
    Ok(agents.status(&engine).await)
}

#[tauri::command]
pub async fn set_agent_account_mode(
    engine: State<'_, Arc<Engine>>,
    agents: State<'_, Agents>,
    account_id: i64,
    mode: Option<AgentMode>,
) -> R {
    engine.set_agent_account_mode(account_id, mode).map_err(|e| format!("{e:#}"))?;
    Ok(agents.status(&engine).await)
}

#[tauri::command]
pub async fn set_agent_account_allowed(
    engine: State<'_, Arc<Engine>>,
    agents: State<'_, Agents>,
    account_id: i64,
    allowed: bool,
) -> R {
    engine.set_agent_account_allowed(account_id, allowed).map_err(|e| format!("{e:#}"))?;
    Ok(agents.status(&engine).await)
}

#[tauri::command]
pub async fn set_agent_folder_allowed(
    engine: State<'_, Arc<Engine>>,
    agents: State<'_, Agents>,
    folder_id: i64,
    allowed: Option<bool>,
) -> R {
    engine.set_agent_folder_allowed(folder_id, allowed).map_err(|e| format!("{e:#}"))?;
    Ok(agents.status(&engine).await)
}

#[tauri::command(async)]
pub fn agent_actions(engine: State<'_, Arc<Engine>>, limit: Option<usize>) -> Result<Vec<AgentAction>, String> {
    engine.agent_actions(limit.unwrap_or(30)).map_err(|e| format!("{e:#}"))
}

#[tauri::command(async)]
pub fn pending_agent_actions_count(engine: State<'_, Arc<Engine>>) -> Result<usize, String> {
    engine.pending_agent_actions_count().map_err(|e| format!("{e:#}"))
}

#[tauri::command(async)]
pub fn approve_agent_action(engine: State<'_, Arc<Engine>>, id: i64) -> Result<AgentAction, String> {
    engine.approve_agent_action(id).map_err(|e| format!("{e:#}"))
}

#[tauri::command(async)]
pub fn reject_agent_action(engine: State<'_, Arc<Engine>>, id: i64) -> Result<(), String> {
    engine.reject_agent_action(id).map_err(|e| format!("{e:#}"))
}

#[tauri::command(async)]
pub fn approve_all_agent_actions(engine: State<'_, Arc<Engine>>) -> Result<BatchOutcome, String> {
    engine.approve_all_agent_actions().map_err(|e| format!("{e:#}"))
}

#[tauri::command(async)]
pub fn reject_all_agent_actions(engine: State<'_, Arc<Engine>>) -> Result<usize, String> {
    engine.reject_all_agent_actions().map_err(|e| format!("{e:#}"))
}
