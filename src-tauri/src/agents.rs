//! Access for coding agents on this Mac (Claude Code, Codex, Opencode): runs the MCP server from
//! `crates/mail-mcp` while the setting is on. Off by default.

use mail_core::Engine;
use mail_mcp::{Server, DEFAULT_PORT};
use std::sync::Arc;
use tauri::State;

/// Settings: "on" when agents may connect, and the token they must present.
const ENABLED: &str = "mcp_enabled";
const TOKEN: &str = "mcp_token";

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
}

fn setting(engine: &Engine, key: &str) -> Option<String> {
    engine.setting(key).ok().flatten().filter(|v| !v.is_empty())
}

impl Agents {
    /// Starts or stops the server to match the settings. The token is read at start,
    /// so this also puts a new token into effect.
    pub async fn apply(&self, engine: &Arc<Engine>) -> AgentAccess {
        let mut st = self.state.lock().await;
        if let Some(old) = st.0.take() {
            old.stop().await;
        }
        st.1 = None;
        let enabled = setting(engine, ENABLED).as_deref() == Some("on");
        let token = setting(engine, TOKEN);
        if let (true, Some(token)) = (enabled, &token) {
            match Server::start(engine.clone(), DEFAULT_PORT, token.clone()).await {
                Ok(server) => st.0 = Some(server),
                Err(e) => st.1 = Some(format!("{e:#}. Another copy of Mailvern or another app may be using it.")),
            }
        }
        AgentAccess { enabled, running: st.0.is_some(), url: mail_mcp::url(DEFAULT_PORT), token: token.filter(|_| enabled), error: st.1.clone() }
    }

    async fn status(&self, engine: &Engine) -> AgentAccess {
        let st = self.state.lock().await;
        let enabled = setting(engine, ENABLED).as_deref() == Some("on");
        AgentAccess { enabled, running: st.0.is_some(), url: mail_mcp::url(DEFAULT_PORT), token: setting(engine, TOKEN).filter(|_| enabled), error: st.1.clone() }
    }
}

type R = Result<AgentAccess, String>;

#[tauri::command]
pub async fn agent_access(engine: State<'_, Arc<Engine>>, agents: State<'_, Agents>) -> R {
    Ok(agents.status(&engine).await)
}

#[tauri::command]
pub async fn set_agent_access(engine: State<'_, Arc<Engine>>, agents: State<'_, Agents>, enabled: bool) -> R {
    if enabled && setting(&engine, TOKEN).is_none() {
        engine.set_setting(TOKEN, &mail_mcp::new_token()).map_err(|e| format!("{e:#}"))?;
    }
    engine.set_setting(ENABLED, if enabled { "on" } else { "off" }).map_err(|e| format!("{e:#}"))?;
    Ok(agents.apply(&engine).await)
}

/// Replaces the token, which cuts off every agent set up with the old one.
#[tauri::command]
pub async fn reset_agent_token(engine: State<'_, Arc<Engine>>, agents: State<'_, Agents>) -> R {
    engine.set_setting(TOKEN, &mail_mcp::new_token()).map_err(|e| format!("{e:#}"))?;
    Ok(agents.apply(&engine).await)
}
