pub mod accounts;
pub mod files;
pub mod messages;
pub mod rules;

pub use accounts::*;
pub use files::*;
pub use messages::*;
pub use rules::*;

use crate::agents;
use mail_core::model::{BootstrapData, View};
use mail_core::Engine;
use std::sync::Arc;
use tauri::State;
use tauri_plugin_opener::OpenerExt;

pub(crate) type E<'a> = State<'a, Arc<Engine>>;
pub(crate) type R<T> = Result<T, String>;

// Plain commands run on the main thread, which also delivers keystrokes to the window.
// Commands marked `(async)` run on Tauri's worker pool so SQLite, Keychain, or file I/O
// never stalls typing or window responsiveness.

pub(crate) fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
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
pub fn ai_key(engine: E, provider: Option<String>) -> R<Option<String>> {
    let name = ai_keychain_name(provider.as_deref())?;
    let key = mail_core::secrets::get(&name).ok().filter(|k| !k.is_empty());
    let _ = engine.set_setting(AI_CONFIGURED, if key.is_some() { "1" } else { "0" });
    Ok(key)
}

/// Stores the key for a provider, or removes it when `key` is empty.
#[tauri::command(async)]
pub fn set_ai_key(engine: E, key: String, provider: Option<String>) -> R<()> {
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
pub(crate) const BOOT_PAGE_LIMIT: i64 = 40;

pub(crate) const STARTUP_SETTING_KEYS: &[&str] = &[
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
pub(crate) struct PrebootBootstrap {
    rx: std::sync::Mutex<Option<std::sync::mpsc::Receiver<R<BootstrapData>>>>,
}

impl PrebootBootstrap {
    pub(crate) fn new(rx: std::sync::mpsc::Receiver<R<BootstrapData>>) -> Self {
        Self { rx: std::sync::Mutex::new(Some(rx)) }
    }

    pub(crate) fn empty() -> Self {
        Self { rx: std::sync::Mutex::new(None) }
    }

    fn take(&self) -> Option<R<BootstrapData>> {
        let rx = self.rx.lock().unwrap_or_else(|e| e.into_inner()).take()?;
        rx.recv().ok()
    }
}

/// Releases background account syncing & MCP startup once the first bootstrap snapshot has been delivered.
pub(crate) struct StartGate {
    opened: std::sync::atomic::AtomicBool,
    notify: tokio::sync::Notify,
}

impl StartGate {
    pub(crate) fn new() -> Self {
        Self {
            opened: std::sync::atomic::AtomicBool::new(false),
            notify: tokio::sync::Notify::new(),
        }
    }

    pub(crate) fn open(&self) {
        if !self.opened.swap(true, std::sync::atomic::Ordering::SeqCst) {
            self.notify.notify_waiters();
        }
    }

    pub(crate) async fn wait(&self) {
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
pub fn bootstrap(
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
pub fn badge_count(engine: E) -> R<i64> {
    engine.badge_count().map_err(err)
}

#[tauri::command(async)]
pub fn mark_badge_seen(engine: E) -> R<()> {
    engine.mark_badge_seen().map_err(err)
}


fn is_restricted_setting(key: &str) -> bool {
    matches!(key, agents::ENABLED | agents::MODE | agents::LEGACY_TOKEN)
}

#[tauri::command(async)]
pub fn set_setting(engine: E, key: String, value: String) -> R<()> {
    if is_restricted_setting(&key) {
        return Err("That setting can only be managed from Agent Settings".into());
    }
    engine.set_setting(&key, &value).map_err(err)
}


#[derive(serde::Serialize)]
pub struct SenderIcon {
    /// A `mailvern-icon://localhost/{domain}` URL served directly as binary bytes.
    url: String,
    /// A BIMI logo, made to fill the avatar. Otherwise a website icon.
    logo: bool,
}

/// The sender's domain logo or website icon, None to show initials.
#[tauri::command]
pub async fn sender_icon(engine: E<'_>, email: String) -> R<Option<SenderIcon>> {
    let Some(domain) = mail_core::icons::icon_domain(&email) else {
        return Ok(None);
    };
    Ok(engine.sender_icon(&email).await.map_err(err)?.map(|i| SenderIcon {
        url: format!("mailvern-icon://localhost/{domain}"),
        logo: i.logo,
    }))
}


#[tauri::command(async)]
pub fn contacts(engine: E, query: String) -> R<Vec<mail_core::model::Addr>> {
    engine.contacts(&query).map_err(err)
}

#[tauri::command(async)]
pub fn sent_recipients(engine: E) -> R<Vec<String>> {
    engine.sent_recipients().map_err(err)
}

#[tauri::command]
pub fn open_url(app: tauri::AppHandle, url: String) -> R<()> {
    let parsed = tauri::Url::parse(url.trim()).map_err(|_| "Unsupported link".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https" | "mailto") {
        return Err("Unsupported link".into());
    }
    app.opener().open_url(parsed.as_str(), None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn hide_window(window: tauri::Window) {
    if window.label() == "main" {
        crate::hide_main_window(&window);
    }
}

