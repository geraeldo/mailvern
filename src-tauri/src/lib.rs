use commands::{err, PrebootBootstrap, StartGate, BOOT_PAGE_LIMIT, R, STARTUP_SETTING_KEYS};
use mail_core::model::{BootstrapData, View};
use mail_core::Engine;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{Emitter, Manager};
use tauri_plugin_opener::OpenerExt;

mod agents;
mod ai_http;
mod commands;
#[cfg(target_os = "macos")]
mod context_menu;

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


const MENU_CLOSE_LAYER: &str = "close-layer";

/// Builds the app menu with a custom ⌘W / Ctrl+W item instead of the predefined Close Window
/// action so the frontend can dismiss open modal sheets before hiding the main window.
fn build_app_menu(app: &tauri::AppHandle) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};

    let close_item = MenuItem::with_id(app, MENU_CLOSE_LAYER, "Close", true, Some("CmdOrCtrl+W"))?;
    let file_menu = Submenu::with_items(app, "File", true, &[&close_item])?;

    let edit_menu = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;

    let view_menu = Submenu::with_items(
        app,
        "View",
        true,
        &[&PredefinedMenuItem::fullscreen(app, None)?],
    )?;

    let window_menu = Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, None)?,
        ],
    )?;

    #[cfg(target_os = "macos")]
    {
        let pkg_name = app.package_info().name.clone();
        let app_menu = Submenu::with_items(
            app,
            pkg_name,
            true,
            &[
                &PredefinedMenuItem::about(app, None, None)?,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::services(app, None)?,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::hide(app, None)?,
                &PredefinedMenuItem::hide_others(app, None)?,
                &PredefinedMenuItem::show_all(app, None)?,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::quit(app, None)?,
            ],
        )?;
        Menu::with_items(app, &[&app_menu, &file_menu, &edit_menu, &view_menu, &window_menu])
    }
    #[cfg(not(target_os = "macos"))]
    {
        Menu::with_items(app, &[&file_menu, &edit_menu, &view_menu, &window_menu])
    }
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

            app.set_menu(build_app_menu(app.handle())?)?;
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
            commands::bootstrap,
            commands::accounts,
            commands::save_account,
            commands::discover_servers,
            commands::saved_searches,
            commands::save_saved_search,
            commands::delete_saved_search,
            commands::reorder_saved_searches,
            commands::delete_account,
            commands::reorder_accounts,
            commands::set_account_alerts,
            commands::reorder_folders,
            commands::create_folder,
            commands::rename_folder,
            commands::delete_folder,
            commands::empty_folder,
            commands::permanent_delete,
            commands::folders,
            commands::smart_counts,
            commands::messages,
            commands::thread,
            commands::search,
            commands::search_server,
            commands::query_mail,
            commands::group_mail,
            commands::summaries,
            commands::attachment_content,
            commands::ai_key,
            commands::set_ai_key,
            ai_http::ai_fetch,
            ai_http::ai_fetch_abort,
            commands::move_to,
            commands::snooze,
            commands::unsnooze,
            commands::cancel_send,
            commands::send_now,
            commands::outbox,
            commands::save_draft,
            commands::delete_draft,
            commands::load_older,
            commands::badge_count,
            commands::mark_badge_seen,
            commands::message,
            commands::rsvp_invite,
            commands::open_ics,
            commands::raw_source,
            commands::export_eml,
            commands::cached_message,
            commands::set_seen,
            commands::mark_read,
            commands::set_flagged,
            commands::archive,
            commands::trash,
            commands::junk,
            commands::not_junk,
            commands::send,
            commands::sync_now,
            commands::sync_view,
            commands::sync_account,
            commands::set_setting,
            commands::open_attachment,
            commands::save_attachment,
            commands::save_all_attachments,
            commands::quicklook_attachment,
            commands::stage_attachment,
            commands::stage_eml,
            commands::copy_text,
            commands::file_info,
            commands::attachment_files,
            commands::contacts,
            commands::sent_recipients,
            commands::sender_icon,
            commands::read_image,
            commands::open_url,
            commands::hide_window,
            agents::agent_access,
            agents::set_agent_access,
            agents::reset_agent_token,
            agents::set_agent_mode,
            agents::set_agent_account_mode,
            agents::set_agent_account_allowed,
            agents::set_agent_folder_allowed,
            agents::agent_actions,
            agents::pending_agent_actions_count,
            agents::approve_agent_action,
            agents::reject_agent_action,
            agents::approve_all_agent_actions,
            agents::reject_all_agent_actions,
            commands::rules,
            commands::save_rule,
            commands::delete_rule,
            commands::reorder_rules,
            commands::set_rule_enabled,
            commands::check_rule,
            commands::test_rule,
            commands::run_rule,
            commands::rule_activity,
            commands::undo_rule_activity,
            commands::rule_facts,
        ])
        .on_menu_event(|app, event| {
            if event.id().as_ref() == MENU_CLOSE_LAYER {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.emit("close-requested", ());
                }
            }
        })
        // Like other Mac mail apps: closing the window (red button, or ⌘W with no modal open)
        // only hides it, so sync and notifications keep running. ⌘Q still quits.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    hide_main_window(window);
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

fn hide_main_window(window: &tauri::Window) {
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
