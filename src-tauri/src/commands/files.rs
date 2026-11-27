use super::{err, E, R};
use mail_core::model::AttachmentContent;
use mail_core::text::{quarantine_file, safe_filename};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tauri_plugin_opener::OpenerExt;

#[tauri::command]
pub async fn attachment_content(engine: E<'_>, id: i64, index: usize, offset: Option<usize>, max_chars: Option<usize>) -> R<AttachmentContent> {
    let max = max_chars.unwrap_or(20_000).clamp(500, 200_000);
    engine.attachment_content(id, index, offset.unwrap_or(0), max).await.map_err(err)
}

/// Atomically creates a new file in `dir` without overwriting existing files or following
/// dangling symlinks (`O_CREAT | O_EXCL`), picking a numbered suffix on collisions.
pub(super) fn write_unique_file(dir: &Path, name: &str, bytes: &[u8]) -> R<PathBuf> {
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
pub async fn open_attachment(app: tauri::AppHandle, engine: E<'_>, id: i64, index: usize) -> R<()> {
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
pub async fn save_attachment(app: tauri::AppHandle, engine: E<'_>, id: i64, index: usize) -> R<String> {
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
pub async fn save_all_attachments(app: tauri::AppHandle, engine: E<'_>, id: i64, dest_dir: Option<String>) -> R<usize> {
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
pub async fn quicklook_attachment(engine: E<'_>, id: i64, index: usize) -> R<()> {
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
pub async fn stage_attachment(engine: E<'_>, id: i64, index: usize) -> R<FileInfo> {
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
pub async fn stage_eml(engine: E<'_>, id: i64) -> R<FileInfo> {
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
pub fn copy_text(text: String) -> R<()> {
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
pub fn copy_text(_text: String) -> R<()> {
    Err("Clipboard write not supported on this platform".into())
}

#[derive(serde::Serialize)]
pub struct FileInfo {
    pub path: String,
    pub name: String,
    pub size: u64,
}

#[tauri::command(async)]
pub fn file_info(paths: Vec<String>) -> Vec<FileInfo> {
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
pub async fn attachment_files(engine: E<'_>, id: i64) -> R<Vec<FileInfo>> {
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
pub fn read_image(path: String) -> R<String> {
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
