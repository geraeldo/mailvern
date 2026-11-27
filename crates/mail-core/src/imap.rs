//! Thin helpers over async-imap: connecting, capabilities, folder discovery (LIST-STATUS),
//! CONDSTORE/UIDPLUS support, and strict tagged-completion response parsing.

use anyhow::{anyhow, Context, Result};
use async_native_tls::TlsStream;
use imap_proto::types::{
    AttributeValue, MailboxDatum, MessageSection, NameAttribute, Response, ResponseCode, SectionPath, Status,
    StatusAttribute, UidSetMember,
};
use socket2::{SockRef, TcpKeepalive};
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use tokio::net::TcpStream;

pub type Session = async_imap::Session<TlsStream<TcpStream>>;

pub const CMD_TIMEOUT: Duration = Duration::from_secs(60);
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(120);
pub const LIVENESS_TIMEOUT: Duration = Duration::from_secs(5);

/// Only the header fields `parse::parse_header` actually uses (5–20x smaller than full `BODY.PEEK[HEADER]`).
pub const HEADER_ITEMS: &str = "(UID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[HEADER.FIELDS (DATE FROM TO CC REPLY-TO SUBJECT MESSAGE-ID IN-REPLY-TO REFERENCES CONTENT-TYPE LIST-ID AUTO-SUBMITTED PRECEDENCE)])";

/// Negotiated IMAP server extensions.
#[derive(Clone, Copy, Debug, Default)]
pub struct ServerCaps {
    pub idle: bool,
    pub condstore: bool,
    pub list_status: bool,
    pub move_ext: bool,
    pub uidplus: bool,
}

/// Mailbox metadata returned by `STATUS` or `LIST-STATUS`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FolderStatus {
    pub messages: u32,
    pub uid_next: Option<u32>,
    pub uid_validity: Option<u32>,
    pub unseen: Option<u32>,
    pub highest_modseq: Option<u64>,
}

/// Mailbox metadata returned by `SELECT` / `EXAMINE`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SelectedMailbox {
    pub exists: u32,
    pub uid_validity: u32,
    pub uid_next: Option<u32>,
    pub highest_modseq: Option<u64>,
    pub nomodseq: bool,
}

/// Parsed header fetch response from `fetch_headers`.
pub struct FetchedHeader {
    pub uid: u32,
    pub seen: bool,
    pub flagged: bool,
    pub internal_date: i64,
    pub size: u32,
    pub header: Vec<u8>,
}

/// Parsed `COPYUID` response code from `UID MOVE` / `UID COPY` (RFC 4315).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopyUid {
    pub uid_validity: u32,
    pub pairs: Vec<(u32, u32)>,
}

pub async fn connect(host: &str, port: u16, user: &str, password: &str) -> Result<Session> {
    let tcp = tokio::time::timeout(Duration::from_secs(20), TcpStream::connect((host, port)))
        .await
        .map_err(|_| anyhow!("Timed out connecting to {host}"))?
        .with_context(|| format!("Can't reach {host}:{port}"))?;
    let _ = tcp.set_nodelay(true);
    // OS-level keepalive detects dead NAT/sleep sockets in ~40–60s instead of hanging until timeout.
    let ka = TcpKeepalive::new()
        .with_time(Duration::from_secs(30))
        .with_interval(Duration::from_secs(10));
    let _ = SockRef::from(&tcp).set_tcp_keepalive(&ka);

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

/// Queries the server's supported IMAP extensions.
pub async fn capabilities<T: Wire>(session: &mut async_imap::Session<T>) -> Result<ServerCaps> {
    let mut caps = ServerCaps::default();
    run_checked(session, CMD_TIMEOUT, "querying capabilities", "CAPABILITY", |resp| {
        if let Response::Capabilities(list) = resp {
            apply_caps(&mut caps, list);
        } else if let Response::Data { code: Some(ResponseCode::Capabilities(list)), .. } = resp {
            apply_caps(&mut caps, list);
        }
    })
    .await?;
    Ok(caps)
}

fn apply_caps(out: &mut ServerCaps, list: &[imap_proto::types::Capability<'_>]) {
    for c in list {
        if let imap_proto::types::Capability::Atom(a) = c {
            if a.eq_ignore_ascii_case("IDLE") {
                out.idle = true;
            } else if a.eq_ignore_ascii_case("CONDSTORE") {
                out.condstore = true;
            } else if a.eq_ignore_ascii_case("LIST-STATUS") {
                out.list_status = true;
            } else if a.eq_ignore_ascii_case("MOVE") {
                out.move_ext = true;
            } else if a.eq_ignore_ascii_case("UIDPLUS") {
                out.uidplus = true;
            }
        }
    }
}

pub struct RemoteFolder {
    pub path: String,
    pub name: String,
    pub role: Option<&'static str>,
    pub status: Option<FolderStatus>,
}

fn attr_role(attrs: &[NameAttribute<'_>]) -> Option<&'static str> {
    for a in attrs {
        match a {
            NameAttribute::Sent => return Some("sent"),
            NameAttribute::Drafts => return Some("drafts"),
            NameAttribute::Trash => return Some("trash"),
            NameAttribute::Junk => return Some("junk"),
            NameAttribute::Archive => return Some("archive"),
            NameAttribute::All => return Some("all"),
            NameAttribute::Extension(s) => {
                let lower = s.to_ascii_lowercase();
                if lower == "\\sent" {
                    return Some("sent");
                } else if lower == "\\drafts" {
                    return Some("drafts");
                } else if lower == "\\trash" {
                    return Some("trash");
                } else if lower == "\\junk" {
                    return Some("junk");
                } else if lower == "\\archive" {
                    return Some("archive");
                } else if lower == "\\all" {
                    return Some("all");
                }
            }
            _ => {}
        }
    }
    None
}

fn is_non_selectable(attrs: &[NameAttribute<'_>]) -> bool {
    attrs.iter().any(|a| match a {
        NameAttribute::NoSelect => true,
        NameAttribute::Extension(s) => {
            s.eq_ignore_ascii_case("\\NonExistent") || s.eq_ignore_ascii_case("\\NoSelect")
        }
        _ => false,
    })
}

fn name_role(path: &str) -> Option<&'static str> {
    let p = path.to_ascii_lowercase();
    let leaf = p.rsplit(['/', '.']).next().unwrap_or(&p);
    if p == "inbox" {
        return Some("inbox");
    }
    if leaf == "snoozed" {
        return Some("snoozed");
    }
    let checks: [(&[&str], &'static str); 5] = [
        (&["sent", "sent items", "sent mail", "sent messages"], "sent"),
        (&["drafts", "draft"], "drafts"),
        (&["trash", "deleted items", "deleted messages", "bin"], "trash"),
        (&["spam", "junk", "junk e-mail", "junk email"], "junk"),
        (&["archive", "archives"], "archive"),
    ];
    for (names, role) in checks {
        if names.contains(&leaf) {
            return Some(role);
        }
    }
    None
}

/// The stream bounds async-imap asks for. `Session` is the real one; tests use an in-memory pipe.
pub trait Wire: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + std::fmt::Debug + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + std::fmt::Debug + Send> Wire for T {}

const INCOMPLETE: &str = "The mail server's answer was cut off";

/// Runs an IMAP command and reads responses until the matching tagged `OK` arrives.
///
/// Unlike `async-imap`'s stream wrappers (`list`, `fetch`, `uid_fetch`), which silently end
/// the stream on EOF, `* BYE`, or tagged `NO`/`BAD`, this guarantees the command completed
/// with a matching tagged `OK` — eliminating both silent truncation and extra `NOOP` round-trips.
async fn run_checked<T: Wire>(
    session: &mut async_imap::Session<T>,
    timeout: Duration,
    label: &str,
    command: &str,
    mut on_resp: impl FnMut(&Response<'_>),
) -> Result<Option<ResponseCode<'static>>> {
    tokio::time::timeout(timeout, async {
        let tag = session.run_command(command).await?;
        while let Some(res) = session.read_response().await {
            let resp = res.map_err(|e| anyhow!("{INCOMPLETE} ({label}: {e})"))?;
            match resp.parsed() {
                Response::Done { tag: rtag, status, code, information } if rtag == &tag => {
                    return match status {
                        Status::Ok => {
                            if let Some(ResponseCode::Capabilities(_)) = code {
                                // ignore
                            }
                            Ok(code.as_ref().map(|c| clone_response_code(c)))
                        }
                        Status::No => {
                            let info = information.as_deref().unwrap_or("");
                            Err(anyhow!("{INCOMPLETE} (No: {info})"))
                        }
                        Status::Bad => {
                            let info = information.as_deref().unwrap_or("");
                            Err(anyhow!("{INCOMPLETE} (Bad: {info})"))
                        }
                        _ => Err(anyhow!("{INCOMPLETE} ({label})")),
                    };
                }
                Response::Data { status: Status::Bye, .. } => {
                    return Err(anyhow!("{INCOMPLETE} (server closed connection)"));
                }
                other => on_resp(other),
            }
        }
        Err(anyhow!("{INCOMPLETE} ({label})"))
    })
    .await
    .map_err(|_| anyhow!("Timed out {label}"))?
}

fn clone_response_code(c: &ResponseCode<'_>) -> ResponseCode<'static> {
    match c {
        ResponseCode::HighestModSeq(m) => ResponseCode::HighestModSeq(*m),
        ResponseCode::UidNext(u) => ResponseCode::UidNext(*u),
        ResponseCode::UidValidity(u) => ResponseCode::UidValidity(*u),
        ResponseCode::Unseen(u) => ResponseCode::Unseen(*u),
        ResponseCode::CopyUid(uv, s, d) => ResponseCode::CopyUid(*uv, s.clone(), d.clone()),
        ResponseCode::AppendUid(uv, u) => ResponseCode::AppendUid(*uv, u.clone()),
        ResponseCode::ReadOnly => ResponseCode::ReadOnly,
        ResponseCode::ReadWrite => ResponseCode::ReadWrite,
        ResponseCode::TryCreate => ResponseCode::TryCreate,
        ResponseCode::Alert => ResponseCode::Alert,
        ResponseCode::Parse => ResponseCode::Parse,
        ResponseCode::UidNotSticky => ResponseCode::UidNotSticky,
        _ => ResponseCode::Alert,
    }
}

/// Fast liveness ping (`NOOP`) used before reusing a session that has sat idle.
pub async fn ping<T: Wire>(session: &mut async_imap::Session<T>) -> Result<()> {
    run_checked(session, LIVENESS_TIMEOUT, "checking connection", "NOOP", |_| {}).await?;
    Ok(())
}

fn parse_status_attrs(attrs: &[StatusAttribute]) -> FolderStatus {
    let mut st = FolderStatus::default();
    for a in attrs {
        match a {
            StatusAttribute::Messages(n) => st.messages = *n,
            StatusAttribute::UidNext(n) => st.uid_next = Some(*n),
            StatusAttribute::UidValidity(n) => st.uid_validity = Some(*n),
            StatusAttribute::Unseen(n) => st.unseen = Some(*n),
            StatusAttribute::HighestModSeq(n) => st.highest_modseq = Some(*n),
            _ => {}
        }
    }
    st
}

/// Lists all selectable folders, optionally fetching STATUS metadata for every folder in the same
/// round-trip via RFC 5819 `LIST-STATUS`. Never returns a partial list.
pub async fn list_folders_with_status<T: Wire>(
    session: &mut async_imap::Session<T>,
    use_list_status: bool,
    condstore: bool,
) -> Result<Vec<RemoteFolder>> {
    let cmd = if use_list_status {
        if condstore {
            "LIST \"\" \"*\" RETURN (STATUS (MESSAGES UIDNEXT UIDVALIDITY UNSEEN HIGHESTMODSEQ))"
        } else {
            "LIST \"\" \"*\" RETURN (STATUS (MESSAGES UIDNEXT UIDVALIDITY UNSEEN))"
        }
    } else {
        "LIST \"\" \"*\""
    };

    let mut raw_folders: Vec<(String, String, Vec<NameAttribute<'static>>)> = Vec::new();
    let mut statuses: HashMap<String, FolderStatus> = HashMap::new();

    let res = run_checked(session, CMD_TIMEOUT, "listing folders", cmd, |resp| match resp {
        Response::MailboxData(MailboxDatum::List { name_attributes, delimiter, name }) => {
            if is_non_selectable(name_attributes) {
                return;
            }
            let path = name.to_string();
            let delim = delimiter.as_deref().unwrap_or("/");
            let leaf = path.rsplit(delim).next().unwrap_or(&path).to_string();
            let owned_attrs = name_attributes.iter().map(|a| clone_name_attr(a)).collect();
            raw_folders.push((path, leaf, owned_attrs));
        }
        Response::MailboxData(MailboxDatum::Status { mailbox, status }) => {
            statuses.insert(mailbox.to_string(), parse_status_attrs(status));
        }
        _ => {}
    })
    .await;

    // If a server advertises LIST-STATUS but rejects the command, fall back to plain LIST.
    if res.is_err() && use_list_status {
        raw_folders.clear();
        statuses.clear();
        run_checked(session, CMD_TIMEOUT, "listing folders", "LIST \"\" \"*\"", |resp| {
            if let Response::MailboxData(MailboxDatum::List { name_attributes, delimiter, name }) = resp {
                if is_non_selectable(name_attributes) {
                    return;
                }
                let path = name.to_string();
                let delim = delimiter.as_deref().unwrap_or("/");
                let leaf = path.rsplit(delim).next().unwrap_or(&path).to_string();
                let owned_attrs = name_attributes.iter().map(|a| clone_name_attr(a)).collect();
                raw_folders.push((path, leaf, owned_attrs));
            }
        })
        .await?;
    } else {
        res?;
    }

    // Two-pass role assignment:
    // Pass 1: INBOX and explicit RFC 6154 SPECIAL-USE attributes take priority.
    let mut claimed: HashSet<&'static str> = HashSet::new();
    let mut out: Vec<RemoteFolder> = Vec::with_capacity(raw_folders.len());
    for (path, leaf, attrs) in raw_folders {
        let role = if path.eq_ignore_ascii_case("inbox") {
            Some("inbox")
        } else {
            attr_role(&attrs)
        };
        if let Some(r) = role {
            claimed.insert(r);
        }
        let status = statuses.remove(&path);
        let name = if role == Some("inbox") { "Inbox".to_string() } else { leaf };
        out.push(RemoteFolder { path, name, role, status });
    }
    // Pass 2: Fall back to folder-name heuristics only for roles not claimed by SPECIAL-USE.
    for f in &mut out {
        if f.role.is_none() {
            if let Some(r) = name_role(&f.path) {
                if r == "snoozed" || !claimed.contains(r) {
                    f.role = Some(r);
                    claimed.insert(r);
                    if r == "inbox" {
                        f.name = "Inbox".to_string();
                    }
                }
            }
        }
    }

    // Every account has an inbox, so a list without one is a refused or empty answer.
    if !out.iter().any(|f| f.role == Some("inbox")) {
        return Err(anyhow!("{INCOMPLETE} (no folder list)"));
    }
    Ok(out)
}

fn clone_name_attr(a: &NameAttribute<'_>) -> NameAttribute<'static> {
    match a {
        NameAttribute::NoInferiors => NameAttribute::NoInferiors,
        NameAttribute::NoSelect => NameAttribute::NoSelect,
        NameAttribute::Marked => NameAttribute::Marked,
        NameAttribute::Unmarked => NameAttribute::Unmarked,
        NameAttribute::All => NameAttribute::All,
        NameAttribute::Archive => NameAttribute::Archive,
        NameAttribute::Drafts => NameAttribute::Drafts,
        NameAttribute::Flagged => NameAttribute::Flagged,
        NameAttribute::Junk => NameAttribute::Junk,
        NameAttribute::Sent => NameAttribute::Sent,
        NameAttribute::Trash => NameAttribute::Trash,
        NameAttribute::Extension(s) => NameAttribute::Extension(std::borrow::Cow::Owned(s.to_string())),
        other => NameAttribute::Extension(std::borrow::Cow::Owned(format!("{other:?}"))),
    }
}

#[cfg(test)]
pub async fn list_folders<T: Wire>(session: &mut async_imap::Session<T>) -> Result<Vec<RemoteFolder>> {
    list_folders_with_status(session, false, false).await
}

/// Creates a mailbox on the server (`CREATE`).
pub async fn create_folder<T: Wire>(session: &mut async_imap::Session<T>, path: &str) -> Result<()> {
    run_checked(session, CMD_TIMEOUT, &format!("creating folder {path}"), &format!("CREATE {}", quote(path)), |_| {}).await?;
    Ok(())
}

/// Renames a mailbox on the server (`RENAME`).
pub async fn rename_folder<T: Wire>(session: &mut async_imap::Session<T>, from: &str, to: &str) -> Result<()> {
    run_checked(
        session,
        CMD_TIMEOUT,
        &format!("renaming folder {from}"),
        &format!("RENAME {} {}", quote(from), quote(to)),
        |_| {},
    )
    .await?;
    Ok(())
}

/// Deletes a mailbox on the server (`DELETE`).
pub async fn delete_folder<T: Wire>(session: &mut async_imap::Session<T>, path: &str) -> Result<()> {
    run_checked(session, CMD_TIMEOUT, &format!("deleting folder {path}"), &format!("DELETE {}", quote(path)), |_| {}).await?;
    Ok(())
}

/// Fetches STATUS metadata for a single mailbox without changing the selected mailbox.
pub async fn status<T: Wire>(session: &mut async_imap::Session<T>, path: &str, condstore: bool) -> Result<FolderStatus> {
    let items = if condstore {
        "(MESSAGES UIDNEXT UIDVALIDITY UNSEEN HIGHESTMODSEQ)"
    } else {
        "(MESSAGES UIDNEXT UIDVALIDITY UNSEEN)"
    };
    let cmd = format!("STATUS {} {items}", quote(path));
    let mut found = None;
    run_checked(session, CMD_TIMEOUT, &format!("checking status of {path}"), &cmd, |resp| {
        if let Response::MailboxData(MailboxDatum::Status { status, .. }) = resp {
            found = Some(parse_status_attrs(status));
        }
    })
    .await?;
    found.ok_or_else(|| anyhow!("{INCOMPLETE} (no STATUS for {path})"))
}

/// Opens a folder for syncing (`SELECT`, with `(CONDSTORE)` when supported).
pub async fn select_folder<T: Wire>(
    session: &mut async_imap::Session<T>,
    path: &str,
    condstore: bool,
) -> Result<SelectedMailbox> {
    let quoted = quote(path);
    let cmd = if condstore {
        format!("SELECT {quoted} (CONDSTORE)")
    } else {
        format!("SELECT {quoted}")
    };
    match open_mailbox(session, &cmd, path).await {
        Ok(mb) => Ok(mb),
        Err(_) if condstore => open_mailbox(session, &format!("SELECT {quoted}"), path).await,
        Err(e) => Err(e),
    }
}

/// Opens a folder read-only (`EXAMINE`) — used by the IDLE watcher connection.
pub async fn examine<T: Wire>(session: &mut async_imap::Session<T>, path: &str) -> Result<SelectedMailbox> {
    open_mailbox(session, &format!("EXAMINE {}", quote(path)), path).await
}

async fn open_mailbox<T: Wire>(session: &mut async_imap::Session<T>, cmd: &str, path: &str) -> Result<SelectedMailbox> {
    let mut exists = 0u32;
    let mut uid_validity = None;
    let mut uid_next = None;
    let mut highest_modseq = None;
    let mut nomodseq = false;

    let done_code = run_checked(session, CMD_TIMEOUT, &format!("opening {path}"), cmd, |resp| match resp {
        Response::MailboxData(MailboxDatum::Exists(n)) => exists = *n,
        Response::Data { status: Status::Ok, code, information } => {
            match code {
                Some(ResponseCode::UidValidity(u)) => uid_validity = Some(*u),
                Some(ResponseCode::UidNext(u)) => uid_next = Some(*u),
                Some(ResponseCode::HighestModSeq(m)) => highest_modseq = Some(*m),
                _ => {}
            }
            if information.as_deref().is_some_and(|i| i.to_ascii_uppercase().contains("NOMODSEQ")) {
                nomodseq = true;
            }
        }
        _ => {}
    })
    .await?;

    if let Some(ResponseCode::HighestModSeq(m)) = done_code {
        highest_modseq = Some(m);
    }
    let Some(uid_validity) = uid_validity else {
        return Err(anyhow!("{INCOMPLETE} (opening {path})"));
    };
    Ok(SelectedMailbox { exists, uid_validity, uid_next, highest_modseq, nomodseq })
}

#[cfg(test)]
pub async fn select<T: Wire>(session: &mut async_imap::Session<T>, path: &str) -> Result<SelectedMailbox> {
    select_folder(session, path, false).await
}

fn parse_flags_from_attrs(attrs: &[AttributeValue<'_>]) -> (bool, bool) {
    let mut seen = false;
    let mut flagged = false;
    for a in attrs {
        if let AttributeValue::Flags(fls) = a {
            for fl in fls {
                if fl.eq_ignore_ascii_case("\\Seen") {
                    seen = true;
                } else if fl.eq_ignore_ascii_case("\\Flagged") {
                    flagged = true;
                }
            }
        }
    }
    (seen, flagged)
}

fn parse_internal_date(raw: &str) -> i64 {
    let s = raw.trim();
    chrono::DateTime::parse_from_str(s, "%e-%b-%Y %H:%M:%S %z")
        .or_else(|_| chrono::DateTime::parse_from_str(s, "%d-%b-%Y %H:%M:%S %z"))
        .map(|d| d.timestamp())
        .unwrap_or(0)
}

/// Fetches compact headers (`HEADER_ITEMS`) by sequence set (`by_uid = false`) or UID set (`by_uid = true`).
pub async fn fetch_headers<T: Wire>(
    session: &mut async_imap::Session<T>,
    set: &str,
    by_uid: bool,
) -> Result<Vec<FetchedHeader>> {
    let cmd = if by_uid {
        format!("UID FETCH {set} {HEADER_ITEMS}")
    } else {
        format!("FETCH {set} {HEADER_ITEMS}")
    };
    let mut out = Vec::new();
    run_checked(session, FETCH_TIMEOUT, "fetching headers", &cmd, |resp| {
        if let Response::Fetch(_, attrs) = resp {
            let mut uid = None;
            let mut size = 0u32;
            let mut internal_date = 0i64;
            let mut header = None;
            for a in attrs {
                match a {
                    AttributeValue::Uid(u) => uid = Some(*u),
                    AttributeValue::Rfc822Size(sz) => size = *sz,
                    AttributeValue::InternalDate(d) => internal_date = parse_internal_date(d),
                    AttributeValue::BodySection {
                        section: Some(SectionPath::Full(MessageSection::Header)),
                        data: Some(bytes),
                        ..
                    }
                    | AttributeValue::Rfc822Header(Some(bytes)) => {
                        header = Some(bytes.as_ref().to_vec());
                    }
                    _ => {}
                }
            }
            if let Some(uid) = uid {
                let (seen, flagged) = parse_flags_from_attrs(attrs);
                out.push(FetchedHeader {
                    uid,
                    seen,
                    flagged,
                    internal_date,
                    size,
                    header: header.unwrap_or_default(),
                });
            }
        }
    })
    .await?;
    Ok(out)
}

/// Fetches full raw RFC 822 message bodies (`(UID BODY.PEEK[])`) for a UID set.
pub async fn fetch_bodies<T: Wire>(
    session: &mut async_imap::Session<T>,
    uid_set: &str,
) -> Result<Vec<(u32, Vec<u8>)>> {
    let cmd = format!("UID FETCH {uid_set} (UID BODY.PEEK[])");
    let mut out = Vec::new();
    run_checked(session, FETCH_TIMEOUT, "fetching bodies", &cmd, |resp| {
        if let Response::Fetch(_, attrs) = resp {
            let mut uid = None;
            let mut body = None;
            for a in attrs {
                match a {
                    AttributeValue::Uid(u) => uid = Some(*u),
                    AttributeValue::BodySection { section: None, data: Some(bytes), .. }
                    | AttributeValue::Rfc822(Some(bytes)) => {
                        body = Some(bytes.as_ref().to_vec());
                    }
                    _ => {}
                }
            }
            if let (Some(uid), Some(body)) = (uid, body) {
                out.push((uid, body));
            }
        }
    })
    .await?;
    Ok(out)
}

/// `(uid, seen, flagged)` for the messages in `set` of the selected folder.
/// With `never_empty`, an empty answer is rejected: `n:*` always includes the newest message
/// in a non-empty folder.
pub async fn fetch_flags<T: Wire>(
    session: &mut async_imap::Session<T>,
    set: String,
    never_empty: bool,
) -> Result<Vec<(u32, bool, bool)>> {
    let cmd = format!("UID FETCH {set} (UID FLAGS)");
    let mut flags = Vec::new();
    run_checked(session, FETCH_TIMEOUT, "fetching flags", &cmd, |resp| {
        if let Response::Fetch(_, attrs) = resp {
            let uid = attrs.iter().find_map(|a| match a {
                AttributeValue::Uid(u) => Some(*u),
                _ => None,
            });
            if let Some(uid) = uid {
                let (seen, flagged) = parse_flags_from_attrs(attrs);
                flags.push((uid, seen, flagged));
            }
        }
    })
    .await?;
    if never_empty && flags.is_empty() {
        return Err(anyhow!("{INCOMPLETE} (no messages listed)"));
    }
    Ok(flags)
}

/// CONDSTORE incremental flag fetch (`CHANGEDSINCE modseq`): returns only messages whose flags changed.
pub async fn fetch_flags_since<T: Wire>(
    session: &mut async_imap::Session<T>,
    set: &str,
    modseq: u64,
) -> Result<Vec<(u32, bool, bool)>> {
    let cmd = format!("UID FETCH {set} (UID FLAGS) (CHANGEDSINCE {modseq})");
    let mut flags = Vec::new();
    run_checked(session, FETCH_TIMEOUT, "fetching changed flags", &cmd, |resp| {
        if let Response::Fetch(_, attrs) = resp {
            let uid = attrs.iter().find_map(|a| match a {
                AttributeValue::Uid(u) => Some(*u),
                _ => None,
            });
            if let Some(uid) = uid {
                let (seen, flagged) = parse_flags_from_attrs(attrs);
                flags.push((uid, seen, flagged));
            }
        }
    })
    .await?;
    Ok(flags)
}

/// Compact `UID SEARCH` verified for tagged `OK` completion.
pub async fn uid_search<T: Wire>(session: &mut async_imap::Session<T>, criteria: &str) -> Result<Vec<u32>> {
    let cmd = format!("UID SEARCH {criteria}");
    let mut uids = Vec::new();
    run_checked(session, CMD_TIMEOUT, "searching messages", &cmd, |resp| {
        if let Response::MailboxData(MailboxDatum::Search(list)) = resp {
            uids.extend_from_slice(list);
        }
    })
    .await?;
    Ok(uids)
}

fn expand_uid_members(members: &[UidSetMember]) -> Vec<u32> {
    let mut out = Vec::new();
    for m in members {
        match m {
            UidSetMember::Uid(u) => out.push(*u),
            UidSetMember::UidRange(r) => {
                // Guard against malformed huge ranges from buggy servers.
                if r.end().saturating_sub(*r.start()) <= 10_000 {
                    out.extend(r.clone());
                }
            }
        }
    }
    out
}

fn extract_copyuid(code: &ResponseCode<'_>) -> Option<CopyUid> {
    if let ResponseCode::CopyUid(uv, src, dst) = code {
        let s = expand_uid_members(src);
        let d = expand_uid_members(dst);
        if s.len() == d.len() && !s.is_empty() {
            return Some(CopyUid {
                uid_validity: *uv,
                pairs: s.into_iter().zip(d).collect(),
            });
        }
    }
    None
}

/// Runs `UID MOVE` (when `use_move`) or `UID COPY`, returning any `COPYUID` mapping from the server.
pub async fn uid_move_or_copy<T: Wire>(
    session: &mut async_imap::Session<T>,
    set: &str,
    dest: &str,
    use_move: bool,
) -> Result<Option<CopyUid>> {
    let verb = if use_move { "UID MOVE" } else { "UID COPY" };
    let cmd = format!("{verb} {set} {}", quote(dest));
    let mut copyuid = None;
    let done_code = run_checked(session, CMD_TIMEOUT, if use_move { "moving messages" } else { "copying messages" }, &cmd, |resp| {
        if let Response::Data { status: Status::Ok, code: Some(c), .. } = resp {
            if let Some(cu) = extract_copyuid(c) {
                copyuid = Some(cu);
            }
        }
    })
    .await?;
    if copyuid.is_none() {
        if let Some(c) = done_code.as_ref() {
            copyuid = extract_copyuid(c);
        }
    }
    Ok(copyuid)
}

/// An IMAP quoted string. Non-ASCII text needs CHARSET UTF-8 on the command (see `search_criteria`).
pub fn quote(s: &str) -> String {
    let clean: String = s.chars().filter(|c| !matches!(*c, '\r' | '\n' | '\0')).collect();
    format!("\"{}\"", clean.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Splits free text into IMAP SEARCH terms, normalizing `×`/`✕`/`✖` to `x` and dropping
/// punctuation-only tokens (such as standalone `-`) that servers ignore or fail to match.
pub(crate) fn search_terms(s: &str) -> Vec<String> {
    crate::db::normalize_fts(s)
        .split_whitespace()
        .map(|t| t.trim_matches('"').to_string())
        .filter(|t| t.chars().any(char::is_alphanumeric))
        .collect()
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
    let words = |s: &Option<String>| -> Vec<String> { s.as_deref().map(search_terms).unwrap_or_default() };
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
        let q = MailQuery { text: Some("Quotation - AQUA × MPL S18".into()), ..Default::default() };
        assert_eq!(
            query_criteria(&q).unwrap(),
            r#"TEXT "Quotation" TEXT "AQUA" TEXT "x" TEXT "MPL" TEXT "S18""#
        );
    }

    #[test]
    fn snoozed_folder_role() {
        assert_eq!(name_role("Snoozed"), Some("snoozed"));
        assert_eq!(name_role("INBOX.Snoozed"), Some("snoozed"));
    }
}

#[cfg(test)]
mod cut_off_answers {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    const FOLDERS: &str = "* LIST (\\HasNoChildren) \"/\" INBOX\r\n* LIST (\\HasNoChildren \\Sent) \"/\" Sent\r\n";
    const FLAGS: &str = "* 1 FETCH (UID 5 FLAGS (\\Seen))\r\n* 2 FETCH (UID 9 FLAGS (\\Flagged))\r\n";
    const MAILBOX: &str = "* 3 EXISTS\r\n* OK [UIDVALIDITY 7] UIDs valid\r\n* OK [UIDNEXT 12] Predicted next UID\r\n* OK [HIGHESTMODSEQ 42] Highest\r\n";

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
                } else if line.contains("SELECT") || line.contains("EXAMINE") {
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
        assert_eq!(
            folders.iter().map(|f| (f.name.as_str(), f.role)).collect::<Vec<_>>(),
            [("Inbox", Some("inbox")), ("Sent", Some("sent"))]
        );
        for mode in ["drop", "bye", "cut", "no", "none"] {
            let err = list_folders(&mut session(mode).await)
                .await
                .err()
                .unwrap_or_else(|| panic!("{mode}: a cut-off list was accepted"));
            assert!(format!("{err:#}").contains("cut off"), "{mode}: {err:#}");
        }
    }

    #[tokio::test]
    async fn flags_are_complete_or_an_error() {
        assert_eq!(fetch_flags(&mut session("ok").await, "5:*".into(), true).await.unwrap(), [(5, true, false), (9, false, true)]);
        for mode in ["drop", "bye", "cut", "no", "none"] {
            let err = fetch_flags(&mut session(mode).await, "5:*".into(), true)
                .await
                .err()
                .unwrap_or_else(|| panic!("{mode}: a cut-off answer was accepted"));
            assert!(format!("{err:#}").contains("cut off"), "{mode}: {err:#}");
        }
        assert!(fetch_flags(&mut session("none").await, "3,4".into(), false).await.unwrap().is_empty());
        assert!(fetch_flags(&mut session("drop").await, "3,4".into(), false).await.is_err());
    }

    #[tokio::test]
    async fn opening_a_folder_is_complete_or_an_error() {
        let mb = select(&mut session("ok").await, "INBOX").await.unwrap();
        assert_eq!((mb.exists, mb.uid_validity, mb.uid_next, mb.highest_modseq), (3, 7, Some(12), Some(42)));
        for mode in ["drop", "bye", "cut", "no", "none"] {
            let err = select(&mut session(mode).await, "INBOX")
                .await
                .err()
                .unwrap_or_else(|| panic!("{mode}: a cut-off answer was accepted"));
            assert!(format!("{err:#}").contains("cut off") || format!("{err:#}").contains("No"), "{mode}: {err:#}");
        }
    }

    #[tokio::test]
    async fn list_status_condstore_copyuid_and_header_fields_work() {
        let (client_io, server_io) = tokio::io::duplex(8192);
        tokio::spawn(async move {
            let (r, mut w) = tokio::io::split(server_io);
            let mut lines = BufReader::new(r).lines();
            let _ = w.write_all(b"* OK ready\r\n").await;
            while let Ok(Some(line)) = lines.next_line().await {
                let tag = line.split(' ').next().unwrap_or("").to_string();
                let reply = if line.contains(" LOGIN ") {
                    format!("{tag} OK\r\n")
                } else if line.contains(" CAPABILITY") {
                    format!("* CAPABILITY IMAP4rev1 IDLE CONDSTORE LIST-STATUS MOVE UIDPLUS\r\n{tag} OK\r\n")
                } else if line.contains(" LIST ") {
                    // Two folders named Archive: one has \Archive, one is custom Work/Archive.
                    format!(
                        "* LIST (\\HasNoChildren) \"/\" INBOX\r\n\
                         * STATUS INBOX (MESSAGES 10 UIDNEXT 50 UIDVALIDITY 7 UNSEEN 2 HIGHESTMODSEQ 99)\r\n\
                         * LIST (\\HasNoChildren \\Archive) \"/\" Archive\r\n\
                         * LIST (\\HasNoChildren) \"/\" \"Work/Archive\"\r\n\
                         {tag} OK done\r\n"
                    )
                } else if line.contains("HEADER.FIELDS") {
                    let hdr = "From: Alice <a@x.com>\r\nSubject: Hi\r\n\r\n";
                    format!(
                        "* 1 FETCH (UID 11 FLAGS (\\Seen) INTERNALDATE \" 5-Nov-2026 10:00:00 +0000\" RFC822.SIZE 1234 BODY[HEADER.FIELDS (DATE FROM)] {{{}}}\r\n{})\r\n{tag} OK\r\n",
                        hdr.len(),
                        hdr
                    )
                } else if line.contains("UID MOVE") {
                    format!("* OK [COPYUID 7 11:12 21:22] Moved\r\n{tag} OK\r\n")
                } else if line.contains("CHANGEDSINCE") {
                    format!("* 1 FETCH (UID 11 FLAGS (\\Seen \\Flagged))\r\n{tag} OK\r\n")
                } else {
                    format!("{tag} OK\r\n")
                };
                let _ = w.write_all(reply.as_bytes()).await;
            }
        });
        let mut s = async_imap::Client::new(client_io).login("u", "p").await.map_err(|(e, _)| e).unwrap();
        let caps = capabilities(&mut s).await.unwrap();
        assert!(caps.idle && caps.condstore && caps.list_status && caps.move_ext && caps.uidplus);

        let folders = list_folders_with_status(&mut s, true, true).await.unwrap();
        assert_eq!(folders.len(), 3);
        assert_eq!(folders[0].status.unwrap().highest_modseq, Some(99));
        // Only the SPECIAL-USE \Archive folder gets role Some("archive"); Work/Archive stays None.
        assert_eq!(folders[1].role, Some("archive"));
        assert_eq!(folders[2].role, None);

        let hdrs = fetch_headers(&mut s, "11", true).await.unwrap();
        assert_eq!(hdrs.len(), 1);
        assert_eq!(hdrs[0].uid, 11);
        assert!(hdrs[0].seen);
        assert!(hdrs[0].internal_date > 0);
        assert!(String::from_utf8_lossy(&hdrs[0].header).contains("Subject: Hi"));

        let cu = uid_move_or_copy(&mut s, "11:12", "Archive", true).await.unwrap().unwrap();
        assert_eq!(cu.uid_validity, 7);
        assert_eq!(cu.pairs, vec![(11, 21), (12, 22)]);

        let changed = fetch_flags_since(&mut s, "1:*", 50).await.unwrap();
        assert_eq!(changed, vec![(11, true, true)]);
    }
}
