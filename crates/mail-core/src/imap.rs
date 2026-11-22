//! Thin helpers over async-imap: connecting, folder discovery and fetch parsing.

use anyhow::{anyhow, Context, Result};
use async_imap::types::Fetch;
use async_native_tls::TlsStream;
use futures::TryStreamExt;
use std::time::Duration;
use tokio::net::TcpStream;

pub type Session = async_imap::Session<TlsStream<TcpStream>>;

pub async fn connect(host: &str, port: u16, user: &str, password: &str) -> Result<Session> {
    let tcp = tokio::time::timeout(Duration::from_secs(20), TcpStream::connect((host, port)))
        .await
        .map_err(|_| anyhow!("Timed out connecting to {host}"))?
        .with_context(|| format!("Can't reach {host}:{port}"))?;
    let tls = async_native_tls::TlsConnector::new()
        .connect(host, tcp)
        .await
        .context("TLS handshake failed")?;
    let client = async_imap::Client::new(tls);
    let session = client
        .login(user, password)
        .await
        .map_err(|(e, _)| anyhow!("Sign-in failed: {e}"))?;
    Ok(session)
}

pub struct RemoteFolder {
    pub path: String,
    pub name: String,
    pub role: Option<&'static str>,
}

fn role_for(path: &str, attrs: &str) -> Option<&'static str> {
    let a = attrs.to_ascii_lowercase();
    let p = path.to_ascii_lowercase();
    let leaf = p.rsplit(['/', '.']).next().unwrap_or(&p);
    if p == "inbox" {
        return Some("inbox");
    }
    let checks: [(&str, &[&str], &'static str); 6] = [
        ("sent", &["sent", "sent items", "sent mail", "sent messages"], "sent"),
        ("drafts", &["drafts", "draft"], "drafts"),
        ("trash", &["trash", "deleted items", "deleted messages", "bin"], "trash"),
        ("junk", &["spam", "junk", "junk e-mail", "junk email"], "junk"),
        ("archive", &["archive", "archives"], "archive"),
        ("all", &[], "all"),
    ];
    for (attr, names, role) in checks {
        if a.contains(attr) || names.contains(&leaf) {
            return Some(role);
        }
    }
    None
}

pub async fn list_folders(session: &mut Session) -> Result<Vec<RemoteFolder>> {
    let names: Vec<_> = session.list(Some(""), Some("*")).await?.try_collect().await?;
    let mut out = Vec::new();
    for n in names {
        let attrs = format!("{:?}", n.attributes());
        if attrs.contains("NoSelect") {
            continue;
        }
        let path = n.name().to_string();
        let delim = n.delimiter().unwrap_or("/");
        let name = path.rsplit(delim).next().unwrap_or(&path).to_string();
        let role = role_for(&path, &attrs);
        let name = if role == Some("inbox") { "Inbox".to_string() } else { name };
        out.push(RemoteFolder { path, name, role });
    }
    Ok(out)
}

pub fn flags_of(f: &Fetch) -> (bool, bool) {
    let mut seen = false;
    let mut flagged = false;
    for fl in f.flags() {
        match fl {
            async_imap::types::Flag::Seen => seen = true,
            async_imap::types::Flag::Flagged => flagged = true,
            _ => {}
        }
    }
    (seen, flagged)
}

/// Compresses a sorted uid list into an IMAP sequence set ("1:5,9,12:14").
pub fn uid_set(uids: &[u32]) -> String {
    let mut v = uids.to_vec();
    v.sort_unstable();
    v.dedup();
    let mut parts = Vec::new();
    let mut i = 0;
    while i < v.len() {
        let start = v[i];
        let mut end = start;
        while i + 1 < v.len() && v[i + 1] == end + 1 {
            i += 1;
            end = v[i];
        }
        parts.push(if start == end { start.to_string() } else { format!("{start}:{end}") });
        i += 1;
    }
    parts.join(",")
}
