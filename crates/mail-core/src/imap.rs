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
    if leaf == "snoozed" {
        return Some("snoozed");
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

/// An IMAP quoted string. Non-ASCII text needs CHARSET UTF-8 on the command (see `search_criteria`).
pub fn quote(s: &str) -> String {
    let clean: String = s.chars().filter(|c| *c != '\r' && *c != '\n').collect();
    format!("\"{}\"", clean.replace('\\', "\\\\").replace('"', "\\\""))
}

/// SEARCH criteria matching every term anywhere in a message (headers or body).
pub fn search_criteria(terms: &[String]) -> String {
    let parts: Vec<String> = terms.iter().map(|t| format!("TEXT {}", quote(t))).collect();
    let q = parts.join(" ");
    if terms.iter().any(|t| !t.is_ascii()) {
        format!("CHARSET UTF-8 {q}")
    } else {
        q
    }
}

/// An IMAP date ("5-Nov-2026") for a unix time, in UTC.
fn imap_date(t: i64) -> Option<String> {
    chrono::DateTime::from_timestamp(t, 0).map(|d| d.format("%-d-%b-%Y").to_string())
}

/// SEARCH criteria for a structured query, or None when it has nothing the server can narrow by
/// (asking for every message would be pointless). Dates are whole days on the server, so they
/// are widened by a day; the local query applies the exact bounds afterwards.
pub fn query_criteria(q: &crate::model::MailQuery) -> Option<String> {
    let words = |s: &Option<String>| -> Vec<String> {
        s.as_deref().unwrap_or("").split_whitespace().map(|t| t.trim_matches('"').to_string()).filter(|t| !t.is_empty()).collect()
    };
    let mut parts = Vec::new();
    let mut texts = Vec::new();
    for t in words(&q.text) {
        parts.push(format!("TEXT {}", quote(&t)));
        texts.push(t);
    }
    for t in words(&q.subject) {
        parts.push(format!("SUBJECT {}", quote(&t)));
        texts.push(t);
    }
    for (key, v) in [("FROM", &q.from), ("TO", &q.to)] {
        if let Some(v) = v.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
            parts.push(format!("{key} {}", quote(v)));
            texts.push(v.to_string());
        }
    }
    let narrows = !parts.is_empty();
    if let Some(d) = q.after.and_then(|t| imap_date(t - 86_400)) {
        parts.push(format!("SINCE {d}"));
    }
    if let Some(d) = q.before.and_then(|t| imap_date(t + 86_400)) {
        parts.push(format!("BEFORE {d}"));
    }
    match q.unread {
        Some(true) => parts.push("UNSEEN".into()),
        Some(false) => parts.push("SEEN".into()),
        None => {}
    }
    match q.flagged {
        Some(true) => parts.push("FLAGGED".into()),
        Some(false) => parts.push("UNFLAGGED".into()),
        None => {}
    }
    // Dates or flags alone would list a whole mailbox: only worth it with words to match.
    if !narrows {
        return None;
    }
    let q = parts.join(" ");
    Some(if texts.iter().any(|t| !t.is_ascii()) { format!("CHARSET UTF-8 {q}") } else { q })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_and_criteria() {
        assert_eq!(quote(r#"a"b\c"#), r#""a\"b\\c""#);
        assert_eq!(search_criteria(&["invoice".into(), "acme".into()]), r#"TEXT "invoice" TEXT "acme""#);
        assert_eq!(search_criteria(&["café".into()]), r#"CHARSET UTF-8 TEXT "café""#);
    }

    #[test]
    fn structured_criteria() {
        use crate::model::MailQuery;
        assert_eq!(query_criteria(&MailQuery { unread: Some(true), ..Default::default() }), None);
        let q = MailQuery {
            text: Some("invoice".into()),
            from: Some("tokopedia".into()),
            after: Some(1_793_836_800), // 2026-11-05 00:00 UTC
            unread: Some(true),
            ..Default::default()
        };
        assert_eq!(query_criteria(&q).unwrap(), r#"TEXT "invoice" FROM "tokopedia" SINCE 4-Nov-2026 UNSEEN"#);
        let q = MailQuery { subject: Some("rapat".into()), to: Some("budi@contoh.id".into()), ..Default::default() };
        assert_eq!(query_criteria(&q).unwrap(), r#"SUBJECT "rapat" TO "budi@contoh.id""#);
    }

    #[test]
    fn snoozed_folder_role() {
        assert_eq!(role_for("Snoozed", ""), Some("snoozed"));
        assert_eq!(role_for("INBOX.Snoozed", ""), Some("snoozed"));
    }
}
