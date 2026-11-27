//! Streaming HTTP proxy for AI provider APIs.
//!
//! Runs requests from Rust via `reqwest` so the WebView is not blocked by
//! provider CORS policies or forced to weaken its CSP `connect-src`. Only
//! HTTPS requests to known AI provider hosts are permitted.

use base64::Engine as _;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use tauri::ipc::Channel;
use tokio::sync::oneshot;

/// Exact HTTPS hosts for all single-API-key providers supported by `@earendil-works/pi-ai`.
const ALLOWED_HOSTS: &[&str] = &[
    "ai-gateway.vercel.sh",
    "api.ant-ling.com",
    "api.anthropic.com",
    "api.cerebras.ai",
    "api.deepseek.com",
    "api.fireworks.ai",
    "api.groq.com",
    "api.kimi.com",
    "api.meta.ai",
    "api.minimax.io",
    "api.minimaxi.com",
    "api.mistral.ai",
    "api.moonshot.ai",
    "api.moonshot.cn",
    "api.openai.com",
    "api.together.ai",
    "api.x.ai",
    "api.xiaomimimo.com",
    "api.z.ai",
    "generativelanguage.googleapis.com",
    "inference.baseten.co",
    "integrate.api.nvidia.com",
    "open.bigmodel.cn",
    "opencode.ai",
    "openrouter.ai",
    "router.huggingface.co",
    "token-plan-ams.xiaomimimo.com",
    "token-plan-cn.xiaomimimo.com",
    "token-plan-sgp.xiaomimimo.com",
    "token-plan.maas.qianwenaiapi.com",
    "token-plan.maas.qwencloudapi.com",
];

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AiFetchEvent {
    Head {
        status: u16,
        headers: Vec<(String, String)>,
    },
    Chunk {
        /// Base64-encoded chunk bytes.
        data: String,
    },
    End,
    Error {
        message: String,
    },
}

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .build()
            .expect("failed to build reqwest client")
    })
}

fn aborts() -> &'static Mutex<HashMap<String, oneshot::Sender<()>>> {
    static ABORTS: OnceLock<Mutex<HashMap<String, oneshot::Sender<()>>>> = OnceLock::new();
    ABORTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn validate_url(raw: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(raw).map_err(|e| format!("Invalid URL: {e}"))?;
    if url.scheme() != "https" {
        return Err("Only HTTPS AI endpoints are allowed.".into());
    }
    let host = url.host_str().unwrap_or("");
    if !ALLOWED_HOSTS.contains(&host) {
        return Err(format!("Host not allowed for AI requests: {host}"));
    }
    Ok(url)
}

#[tauri::command]
pub async fn ai_fetch(
    request_id: String,
    url: String,
    method: String,
    headers: Vec<(String, String)>,
    body: Option<String>,
    body_base64: Option<bool>,
    on_event: Channel<AiFetchEvent>,
) -> Result<(), String> {
    let parsed = validate_url(&url)?;
    let http_method = reqwest::Method::from_bytes(method.as_bytes())
        .map_err(|e| format!("Invalid HTTP method: {e}"))?;

    let mut req = client().request(http_method, parsed);
    for (k, v) in headers {
        // Strip browser-only pseudo/forbidden headers if present.
        if k.eq_ignore_ascii_case("host") || k.eq_ignore_ascii_case("content-length") {
            continue;
        }
        req = req.header(&k, &v);
    }

    if let Some(b) = body {
        if body_base64.unwrap_or(false) {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b)
                .map_err(|e| format!("Invalid base64 body: {e}"))?;
            req = req.body(bytes);
        } else {
            req = req.body(b);
        }
    }

    let (tx, mut rx) = oneshot::channel::<()>();
    aborts().lock().unwrap().insert(request_id.clone(), tx);

    let result = tokio::select! {
        _ = &mut rx => {
            Err("Request aborted".to_string())
        }
        res = async {
            let mut resp = req.send().await.map_err(|e| e.to_string())?;
            let status = resp.status().as_u16();
            let resp_headers: Vec<(String, String)> = resp
                .headers()
                .iter()
                .filter_map(|(k, v)| v.to_str().ok().map(|s| (k.as_str().to_string(), s.to_string())))
                .collect();
            let _ = on_event.send(AiFetchEvent::Head { status, headers: resp_headers });

            while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
                if !chunk.is_empty() {
                    let encoded = base64::engine::general_purpose::STANDARD.encode(&chunk);
                    let _ = on_event.send(AiFetchEvent::Chunk { data: encoded });
                }
            }
            let _ = on_event.send(AiFetchEvent::End);
            Ok::<(), String>(())
        } => res
    };

    aborts().lock().unwrap().remove(&request_id);

    if let Err(message) = result {
        let _ = on_event.send(AiFetchEvent::Error { message });
    }
    Ok(())
}

#[tauri::command]
pub fn ai_fetch_abort(request_id: String) {
    if let Some(tx) = aborts().lock().unwrap().remove(&request_id) {
        let _ = tx.send(());
    }
}
