//! Builds the message for `create_draft`, shaped like one written in the app's composer:
//! the same wrapper, signature and reply quote, so it opens there for editing as usual.

use crate::fmt::who;
use anyhow::{anyhow, bail, Context, Result};
use chrono::{Local, TimeZone};
use mail_core::model::{Account, Addr, MessageDetail, Outgoing};
use mail_core::text::{body_text, escape_html, html_to_text, quotable_html};
use serde::Deserialize;
use std::path::{Component, PathBuf};

// Kept in step with `emailWrap` and `quoteReply` in src/lib/html.ts. The composer
// recognises the wrapper by its style when it reopens a draft.
const WRAP_STYLE: &str = "font-family:-apple-system,BlinkMacSystemFont,'Helvetica Neue',Helvetica,Arial,sans-serif;font-size:14px;line-height:1.55;color:#1d1c1a";
const QUOTE_STYLE: &str = "margin:0 0 0 2px;padding-left:12px;border-left:2px solid #d9d6cf;color:#55524c";
const QUOTE_HEAD_STYLE: &str = "color:#8f8b83;font-size:13px;margin-bottom:6px";

const MAX_ATTACHMENTS: u64 = 25 * 1024 * 1024;

#[derive(Deserialize, Default, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct DraftArgs {
    pub account_id: Option<i64>,
    pub to: Option<Vec<String>>,
    pub cc: Option<Vec<String>>,
    pub bcc: Vec<String>,
    pub subject: Option<String>,
    pub body: String,
    pub attachments: Vec<String>,
    pub reply_to: Option<i64>,
    pub reply_all: bool,
    pub draft_id: Option<String>,
}

fn formatted(a: &Addr) -> String {
    who(a.name.as_deref(), &a.email)
}

fn re(subject: &str) -> String {
    if subject.trim_start().to_lowercase().starts_with("re:") {
        subject.to_string()
    } else {
        format!("Re: {subject}")
    }
}

/// Escapes text for HTML and turns bare web addresses in it into links.
fn linked(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = ["http://", "https://"].iter().filter_map(|p| rest.find(p)).min() {
        // Only at the start of a word: "xhttp://" is not a link.
        if rest[..at].chars().last().is_some_and(|c| c.is_alphanumeric()) {
            out.push_str(&escape_html(&rest[..at + 4]));
            rest = &rest[at + 4..];
            continue;
        }
        let end = rest[at..].find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"')).map_or(rest.len(), |i| at + i);
        let raw = rest[at..end].trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '\'']);
        let url = escape_html(raw);
        out.push_str(&escape_html(&rest[..at]));
        out.push_str(&format!("<a href=\"{url}\">{url}</a>"));
        rest = &rest[at + raw.len()..];
    }
    out.push_str(&escape_html(rest));
    out
}

/// Plain text as the composer's HTML: one block per line.
fn text_to_blocks(text: &str) -> String {
    text.trim()
        .lines()
        .map(|l| if l.trim().is_empty() { "<div><br></div>".to_string() } else { format!("<div>{}</div>", linked(l.trim_end())) })
        .collect()
}

/// A file the agent asked to attach, checked. Mailvern reads it with the user's own
/// permissions, so hidden files (keys, tokens, .env) are refused: a draft must not become
/// a way to read what the agent itself may not.
fn attachment(path: &str) -> Result<(String, u64)> {
    let p = match path.strip_prefix("~/") {
        Some(rest) => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(rest),
        None => PathBuf::from(path),
    };
    if !p.is_absolute() {
        bail!("Attachment paths must be absolute: {path}");
    }
    let real = p.canonicalize().with_context(|| format!("Can't find {path}"))?;
    let meta = std::fs::metadata(&real).with_context(|| format!("Can't read {path}"))?;
    if !meta.is_file() {
        bail!("{path} is not a file");
    }
    let hidden = real.components().any(|c| matches!(c, Component::Normal(n) if n.to_string_lossy().starts_with('.')));
    if hidden {
        bail!("Mailvern doesn't attach hidden files or files in hidden folders ({path}). The user can attach it themselves in Mailvern.");
    }
    Ok((real.to_string_lossy().into_owned(), meta.len()))
}

/// The draft as the engine takes it. `source` is the message being replied to;
/// `mine` are the user's own addresses, left out of a reply-all.
pub fn compose(args: &DraftArgs, account: &Account, mine: &[String], source: Option<&MessageDetail>) -> Result<Outgoing> {
    let is_me = |email: &str| mine.iter().any(|m| m.eq_ignore_ascii_case(email));
    let (mut to, mut cc, mut subject) = (Vec::new(), Vec::new(), String::new());
    let (mut in_reply_to, mut references) = (None, None);
    if let Some(src) = source {
        let sender = if src.reply_to.is_empty() {
            vec![Addr { name: Some(src.summary.from_name.clone()), email: src.summary.from_addr.clone() }]
        } else {
            src.reply_to.clone()
        };
        to = sender.iter().map(formatted).collect();
        if args.reply_all {
            let seen = |email: &str| sender.iter().any(|s| s.email.eq_ignore_ascii_case(email));
            to.extend(src.to.iter().filter(|a| !is_me(&a.email) && !seen(&a.email)).map(formatted));
            cc = src.cc.iter().filter(|a| !is_me(&a.email)).map(formatted).collect();
        }
        subject = re(&src.summary.subject);
        if let Some(id) = src.message_id.as_deref().filter(|s| !s.is_empty()) {
            in_reply_to = Some(id.to_string());
            references = Some([src.references.as_deref().unwrap_or(""), id].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" "));
        }
    }
    if let Some(t) = &args.to {
        to = t.clone();
    }
    if let Some(c) = &args.cc {
        cc = c.clone();
    }
    if let Some(s) = &args.subject {
        subject = s.trim().to_string();
    }

    let mut html = text_to_blocks(&args.body);
    let mut text = args.body.trim().to_string();
    if !account.signature.trim().is_empty() {
        html.push_str(&format!("<div><br>{}</div>", account.signature));
        text.push_str(&format!("\n\n{}", html_to_text(&account.signature)));
    }
    if let Some(src) = source {
        let s = &src.summary;
        let date = Local.timestamp_opt(s.date, 0).single().map(|d| d.format("%a, %b %-d, %Y, %-I:%M %p").to_string()).unwrap_or_default();
        let head = format!("On {date}, {} wrote:", who(Some(&s.from_name), &s.from_addr));
        let original = match src.html.as_deref() {
            Some(h) => quotable_html(h),
            None => format!("<div style=\"white-space:pre-wrap\">{}</div>", linked(src.text.as_deref().unwrap_or(""))),
        };
        html.push_str(&format!(
            "<br><div style=\"{QUOTE_HEAD_STYLE}\">{}</div><blockquote type=\"cite\" style=\"{QUOTE_STYLE}\">{original}</blockquote>",
            escape_html(&head)
        ));
        let quoted: Vec<String> = body_text(src.html.as_deref(), src.text.as_deref(), false).lines().map(|l| format!("> {l}")).collect();
        text.push_str(&format!("\n\n{head}\n{}", quoted.join("\n")));
    }

    let mut attachments = Vec::new();
    let mut size = 0;
    for path in &args.attachments {
        let (real, bytes) = attachment(path)?;
        size += bytes;
        attachments.push(real);
    }
    if size > MAX_ATTACHMENTS {
        return Err(anyhow!("The attachments are {} MB in all; the limit is 25 MB.", size / 1024 / 1024));
    }

    Ok(Outgoing {
        account_id: account.id,
        to,
        cc,
        bcc: args.bcc.clone(),
        subject,
        html: format!("<div style=\"{WRAP_STYLE}\">{html}</div>"),
        text,
        attachments,
        in_reply_to,
        references,
        message_id: args.draft_id.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mail_core::model::MessageSummary;

    fn account() -> Account {
        Account {
            id: 7,
            name: "Alex Rivera".into(),
            email: "alex@acme.co".into(),
            imap_host: String::new(),
            imap_port: 993,
            smtp_host: String::new(),
            smtp_port: 465,
            color: String::new(),
            signature: "<b>Alex</b><br>Acme".into(),
            position: 0,
            label: String::new(),
        }
    }

    fn source() -> MessageDetail {
        let a = |n: &str, e: &str| Addr { name: Some(n.to_string()).filter(|n| !n.is_empty()), email: e.into() };
        MessageDetail {
            summary: MessageSummary {
                id: 42,
                account_id: 7,
                folder_id: 1,
                from_name: "Maya Chen".into(),
                from_addr: "maya@northwind.io".into(),
                subject: "Contract v3".into(),
                snippet: String::new(),
                date: 1_793_000_000,
                seen: true,
                flagged: false,
                has_attachments: false,
                thread_id: None,
                count: 1,
                ids: vec![42],
                snoozed_until: None,
            },
            to: vec![a("Alex", "ALEX@acme.co"), a("Tom", "tom@okafor.dev")],
            cc: vec![a("", "priya@acme.io"), a("", "alex@acme.co")],
            bcc: vec![],
            reply_to: vec![],
            message_id: Some("<m3@northwind.io>".into()),
            in_reply_to: None,
            references: Some("<m1@northwind.io> <m2@northwind.io>".into()),
            html: Some("<html><head><style>p{}</style></head><body><p>Updated &amp; attached.</p></body></html>".into()),
            text: None,
            attachments: vec![],
        }
    }

    #[test]
    fn new_message_has_blocks_links_and_signature() {
        let args = DraftArgs { to: Some(vec!["Tom <tom@okafor.dev>".into()]), subject: Some(" Hello ".into()), body: "Hi Tom,\n\nSee https://acme.co/a?b=1&c=2.\nThanks <3".into(), ..Default::default() };
        let out = compose(&args, &account(), &["alex@acme.co".into()], None).unwrap();
        assert_eq!(out.account_id, 7);
        assert_eq!(out.to, ["Tom <tom@okafor.dev>"]);
        assert_eq!(out.subject, "Hello");
        assert_eq!(
            out.html,
            format!(
                "<div style=\"{WRAP_STYLE}\"><div>Hi Tom,</div><div><br></div><div>See <a href=\"https://acme.co/a?b=1&amp;c=2\">https://acme.co/a?b=1&amp;c=2</a>.</div><div>Thanks &lt;3</div><div><br><b>Alex</b><br>Acme</div></div>"
            )
        );
        assert_eq!(out.text, "Hi Tom,\n\nSee https://acme.co/a?b=1&c=2.\nThanks <3\n\nAlex\nAcme");
        assert!(out.in_reply_to.is_none() && out.references.is_none() && out.message_id.is_none());
    }

    #[test]
    fn reply_all_addresses_threads_and_quotes() {
        let mine = ["alex@acme.co".to_string()];
        let args = DraftArgs { body: "Looks good.".into(), reply_all: true, ..Default::default() };
        let out = compose(&args, &account(), &mine, Some(&source())).unwrap();
        assert_eq!(out.to, ["Maya Chen <maya@northwind.io>", "Tom <tom@okafor.dev>"]);
        assert_eq!(out.cc, ["priya@acme.io"]);
        assert_eq!(out.subject, "Re: Contract v3");
        assert_eq!(out.in_reply_to.as_deref(), Some("<m3@northwind.io>"));
        assert_eq!(out.references.as_deref(), Some("<m1@northwind.io> <m2@northwind.io> <m3@northwind.io>"));
        assert!(out.html.contains("Maya Chen &lt;maya@northwind.io&gt; wrote:</div><blockquote type=\"cite\""));
        assert!(out.html.ends_with("<p>Updated &amp; attached.</p></blockquote></div>"));
        assert!(!out.html.contains("<style>"));
        assert!(out.text.ends_with("wrote:\n> Updated & attached."));

        // A plain reply goes to the sender only; given recipients and subject win.
        let plain = compose(&DraftArgs { body: "Ok".into(), ..Default::default() }, &account(), &mine, Some(&source())).unwrap();
        assert_eq!(plain.to, ["Maya Chen <maya@northwind.io>"]);
        assert!(plain.cc.is_empty());
        let over = DraftArgs { body: "Ok".into(), to: Some(vec!["x@y.co".into()]), subject: Some("Other".into()), ..Default::default() };
        let out = compose(&over, &account(), &mine, Some(&source())).unwrap();
        assert_eq!((out.to, out.subject.as_str()), (vec!["x@y.co".to_string()], "Other"));
        assert_eq!(re("RE: hi"), "RE: hi");
    }

    #[test]
    fn attachments_are_checked() {
        let dir = std::env::temp_dir().join(format!("mailvern-mcp-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join(".secrets")).unwrap();
        std::fs::write(dir.join("report.txt"), "hello").unwrap();
        std::fs::write(dir.join(".env"), "KEY=1").unwrap();
        std::fs::write(dir.join(".secrets/key.pem"), "x").unwrap();
        let p = |name: &str| dir.join(name).to_string_lossy().into_owned();
        let with = |paths: Vec<String>| compose(&DraftArgs { attachments: paths, ..Default::default() }, &account(), &[], None);

        let out = with(vec![p("report.txt")]).unwrap();
        assert!(out.attachments[0].ends_with("/report.txt"));
        assert!(with(vec![p(".env")]).unwrap_err().to_string().contains("hidden"));
        assert!(with(vec![p(".secrets/key.pem")]).unwrap_err().to_string().contains("hidden"));
        assert!(with(vec!["report.txt".into()]).unwrap_err().to_string().contains("absolute"));
        assert!(with(vec![p("missing.txt")]).unwrap_err().to_string().contains("Can't find"));
        assert!(with(vec![dir.to_string_lossy().into_owned()]).unwrap_err().to_string().contains("not a file"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
