//! Thin helpers over async-imap: connecting, folder discovery and fetch parsing.

use anyhow::{anyhow, Context, Result};
use async_imap::types::Fetch;
use async_native_tls::TlsStream;
use futures::TryStreamExt;
use std::time::Duration;
use tokio::net::TcpStream;

pub type Session = async_imap::Session<TlsStream<TcpStream>>;

pub const CMD_TIMEOUT: Duration = Duration::from_secs(60);
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(120);

pub async fn connect(host: &str, port: u16, user: &str, password: &str) -> Result<Session> {
    let tcp = tokio::time::timeout(Duration::from_secs(20), TcpStream::connect((host, port)))
        .await
        .map_err(|_| anyhow!("Timed out connecting to {host}"))?
        .with_context(|| format!("Can't reach {host}:{port}"))?;
    let tls = tokio::time::timeout(Duration::from_secs(20), async_native_tls::TlsConnector::new().connect(host, tcp))
        .await
        .map_err(|_| anyhow!("Timed out during TLS handshake with {host}"))?
        .context("TLS handshake failed")?;
    let client = async_imap::Client::new(tls);
    let session = tokio::time::timeout(Duration::from_secs(30), client.login(user, password))
        .await
        .map_err(|_| anyhow!("Timed out signing in to {host}"))?
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

/// The stream bounds async-imap asks for. `Session` is the real one; tests use an in-memory pipe.
pub trait Wire: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + std::fmt::Debug + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + std::fmt::Debug + Send> Wire for T {}

const INCOMPLETE: &str = "The mail server's answer was cut off";

/// Fails if the connection is gone.
///
/// async-imap ends LIST, FETCH and SEARCH results quietly, as if complete, when the connection
/// drops part-way or the server refuses the command. A short answer then looks like "the rest
/// was deleted". Anything that deletes local mail based on what an answer leaves out must
/// check it first: this catches the drop, and the callers below also reject answers that
/// can't be complete.
pub(crate) async fn confirm_alive<T: Wire>(session: &mut async_imap::Session<T>) -> Result<()> {
    tokio::time::timeout(CMD_TIMEOUT, session.noop())
        .await
        .map_err(|_| anyhow!("Timed out waiting for mail server"))?
        .map(|_| ())
        .context(INCOMPLETE)
}

/// The account's folders. Never a partial list: the caller removes local folders that
/// are missing from it, and their mail with them.
pub async fn list_folders<T: Wire>(session: &mut async_imap::Session<T>) -> Result<Vec<RemoteFolder>> {
    let names: Vec<_> = tokio::time::timeout(CMD_TIMEOUT, async {
        session.list(Some(""), Some("*")).await?.try_collect().await
    })
    .await
    .map_err(|_| anyhow!("Timed out listing folders"))??;
    confirm_alive(session).await?;
    let mut out = Vec::new();
    for n in names {
        let attrs = format!("{:?}", n.attributes());
        if attrs.contains("NoSelect") || attrs.contains("NonExistent") {
            continue;
        }
        let path = n.name().to_string();
        let delim = n.delimiter().unwrap_or("/");
        let name = path.rsplit(delim).next().unwrap_or(&path).to_string();
        let role = role_for(&path, &attrs);
        let name = if role == Some("inbox") { "Inbox".to_string() } else { name };
        out.push(RemoteFolder { path, name, role });
    }
    // Every account has an inbox, so a list without one is a refused or empty answer.
    if !out.iter().any(|f| f.role == Some("inbox")) {
        return Err(anyhow!("{INCOMPLETE} (no folder list)"));
    }
    Ok(out)
}

/// Opens a folder for syncing. The caller empties the local folder when this says the folder
/// is empty or was rebuilt (new UIDVALIDITY), so a cut-off answer is an error. A real answer
/// always carries UIDVALIDITY.
pub async fn select<T: Wire>(session: &mut async_imap::Session<T>, path: &str) -> Result<async_imap::types::Mailbox> {
    let mb = tokio::time::timeout(CMD_TIMEOUT, session.select(path))
        .await
        .map_err(|_| anyhow!("Timed out opening {path}"))??;
    confirm_alive(session).await?;
    if mb.uid_validity.is_none() {
        return Err(anyhow!("{INCOMPLETE} (opening {path})"));
    }
    Ok(mb)
}

/// (uid, seen, flagged) for the messages in `set` of the selected folder. The caller deletes
/// local messages that are missing from it, so a cut-off answer is an error.
///
/// With `never_empty`, an empty answer is one too: "n:*" always includes the folder's newest
/// message, so for a folder that has mail it can't come back empty.
pub async fn fetch_flags<T: Wire>(session: &mut async_imap::Session<T>, set: String, never_empty: bool) -> Result<Vec<(u32, bool, bool)>> {
    let fetches: Vec<Fetch> = tokio::time::timeout(FETCH_TIMEOUT, async {
        session.uid_fetch(set, "(UID FLAGS)").await?.try_collect().await
    })
    .await
    .map_err(|_| anyhow!("Timed out fetching flags"))??;
    confirm_alive(session).await?;
    let flags: Vec<(u32, bool, bool)> = fetches
        .iter()
        .filter_map(|f| {
            let (seen, flagged) = flags_of(f);
            Some((f.uid?, seen, flagged))
        })
        .collect();
    if never_empty && flags.is_empty() {
        return Err(anyhow!("{INCOMPLETE} (no messages listed)"));
    }
    Ok(flags)
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
    let clean: String = s.chars().filter(|c| !matches!(*c, '\r' | '\n' | '\0')).collect();
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
    if let Some(d) = q.after.and_then(|t| imap_date(t.saturating_sub(86_400))) {
        parts.push(format!("SINCE {d}"));
    }
    if let Some(d) = q.before.and_then(|t| imap_date(t.saturating_add(86_400))) {
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

#[cfg(test)]
mod cut_off_answers {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    const FOLDERS: &str = "* LIST (\\HasNoChildren) \"/\" INBOX\r\n* LIST (\\HasNoChildren \\Sent) \"/\" Sent\r\n";
    const FLAGS: &str = "* 1 FETCH (UID 5 FLAGS (\\Seen))\r\n* 2 FETCH (UID 9 FLAGS (\\Flagged))\r\n";
    const MAILBOX: &str = "* 3 EXISTS\r\n* OK [UIDVALIDITY 7] UIDs valid\r\n";

    /// A signed-in session with a server that answers LIST and FETCH as `mode` says:
    /// "ok" in full, "drop" by closing the connection, "bye" by logging the client out,
    /// "cut" by closing part-way through the answer, "no" by refusing, "none" with nothing.
    async fn session(mode: &'static str) -> async_imap::Session<tokio::io::DuplexStream> {
        let (client_io, server_io) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            let (r, mut w) = tokio::io::split(server_io);
            let mut lines = BufReader::new(r).lines();
            let _ = w.write_all(b"* OK ready\r\n").await;
            while let Ok(Some(line)) = lines.next_line().await {
                let tag = line.split(' ').next().unwrap_or("").to_string();
                let data = if line.contains("LIST") {
                    FOLDERS
                } else if line.contains("SELECT") {
                    MAILBOX
                } else {
                    FLAGS
                };
                let reply = if line.contains("LOGIN") || line.contains("NOOP") {
                    format!("{tag} OK\r\n")
                } else {
                    match mode {
                        "ok" => format!("{data}{tag} OK done\r\n"),
                        "none" => format!("{tag} OK done\r\n"),
                        "no" => format!("{tag} NO [UNAVAILABLE] try later\r\n"),
                        "bye" => "* BYE Autologout; idle for too long\r\n".to_string(),
                        "cut" => data.split_inclusive("\r\n").next().unwrap().to_string(),
                        _ => String::new(),
                    }
                };
                let _ = w.write_all(reply.as_bytes()).await;
                if matches!(mode, "drop" | "bye" | "cut") && !line.contains("LOGIN") {
                    return;
                }
            }
        });
        async_imap::Client::new(client_io).login("u", "p").await.map_err(|(e, _)| e).unwrap()
    }

    #[tokio::test]
    async fn a_folder_list_is_complete_or_an_error() {
        let folders = list_folders(&mut session("ok").await).await.unwrap();
        assert_eq!(folders.iter().map(|f| (f.name.as_str(), f.role)).collect::<Vec<_>>(), [("Inbox", Some("inbox")), ("Sent", Some("sent"))]);
        // Each of these used to come back as an empty or short list, and the sync then
        // deleted every folder that was missing from it, with all its mail.
        for mode in ["drop", "bye", "cut", "no", "none"] {
            let err = list_folders(&mut session(mode).await).await.err().unwrap_or_else(|| panic!("{mode}: a cut-off list was accepted"));
            assert!(format!("{err:#}").contains("cut off"), "{mode}: {err:#}");
        }
    }

    #[tokio::test]
    async fn flags_are_complete_or_an_error() {
        assert_eq!(fetch_flags(&mut session("ok").await, "5:*".into(), true).await.unwrap(), [(5, true, false), (9, false, true)]);
        for mode in ["drop", "bye", "cut", "no", "none"] {
            let err = fetch_flags(&mut session(mode).await, "5:*".into(), true).await.err().unwrap_or_else(|| panic!("{mode}: a cut-off answer was accepted"));
            assert!(format!("{err:#}").contains("cut off"), "{mode}: {err:#}");
        }
        // A chosen set of uids may all be gone, so there an empty answer stands; a dropped one doesn't.
        assert!(fetch_flags(&mut session("none").await, "3,4".into(), false).await.unwrap().is_empty());
        assert!(fetch_flags(&mut session("drop").await, "3,4".into(), false).await.is_err());
    }

    #[tokio::test]
    async fn opening_a_folder_is_complete_or_an_error() {
        let mb = select(&mut session("ok").await, "INBOX").await.unwrap();
        assert_eq!((mb.exists, mb.uid_validity), (3, Some(7)));
        // "No messages" or "new UIDVALIDITY" empties the local folder, and a dropped
        // connection used to look like both.
        for mode in ["drop", "bye", "cut", "no", "none"] {
            let err = select(&mut session(mode).await, "INBOX").await.err().unwrap_or_else(|| panic!("{mode}: a cut-off answer was accepted"));
            assert!(format!("{err:#}").contains("cut off") || format!("{err:#}").contains("No"), "{mode}: {err:#}");
        }
    }
}
