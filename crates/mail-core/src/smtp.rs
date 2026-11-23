use crate::model::{Account, Outgoing};
use anyhow::{anyhow, Context, Result};
use lettre::message::header::ContentType;
use base64::Engine as _;
use lettre::address::Envelope;
use lettre::message::{Attachment, Mailbox, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{Address, AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use std::path::Path;
use std::time::Duration;

fn transport(acc: &Account, password: &str) -> Result<AsyncSmtpTransport<Tokio1Executor>> {
    let builder = if acc.smtp_port == 465 {
        AsyncSmtpTransport::<Tokio1Executor>::relay(&acc.smtp_host)?
    } else {
        AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&acc.smtp_host)?
    };
    Ok(builder
        .port(acc.smtp_port)
        .credentials(Credentials::new(acc.email.clone(), password.to_string()))
        .timeout(Some(Duration::from_secs(30)))
        .build())
}

pub async fn test(acc: &Account, password: &str) -> Result<()> {
    let ok = transport(acc, password)?.test_connection().await.context("Outgoing server")?;
    if ok {
        Ok(())
    } else {
        Err(anyhow!("Outgoing server refused the connection"))
    }
}

fn mailbox(s: &str) -> Result<Mailbox> {
    s.trim().parse::<Mailbox>().map_err(|_| anyhow!("Invalid address: {s}"))
}

pub struct InlineImage {
    pub cid: String,
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// Replaces `src="data:image/...;base64,..."` with `cid:` references so mail apps
/// show the images inline. Returns the rewritten HTML and the extracted images.
pub fn extract_inline_images(html: &str, domain: &str) -> (String, Vec<InlineImage>) {
    let mut out = String::with_capacity(html.len());
    let mut images = Vec::new();
    let mut rest = html;
    while let Some(pos) = rest.find("data:image/") {
        let (before, tail) = rest.split_at(pos);
        let quote = before.chars().last();
        let end = match quote {
            Some(q @ ('"' | '\'')) => tail.find(q),
            _ => tail.find(|c: char| c.is_whitespace() || c == '>' || c == ')'),
        };
        let Some(end) = end else { break };
        let uri = &tail[..end];
        let decoded = uri.split_once(";base64,").and_then(|(head, data)| {
            let mime = head.trim_start_matches("data:").to_string();
            let clean: String = data.chars().filter(|c| !c.is_whitespace()).collect();
            base64::engine::general_purpose::STANDARD.decode(clean).ok().map(|b| (mime, b))
        });
        out.push_str(before);
        match decoded {
            Some((mime, bytes)) => {
                let cid = format!("{}@{}", uuid::Uuid::new_v4(), domain);
                out.push_str("cid:");
                out.push_str(&cid);
                images.push(InlineImage { cid, mime, bytes });
            }
            None => out.push_str(uri),
        }
        rest = &tail[end..];
    }
    out.push_str(rest);
    (out, images)
}

/// A fresh Message-ID on the account's domain.
pub fn new_message_id(acc: &Account) -> String {
    let domain = acc.email.split('@').nth(1).unwrap_or("mailvern.local");
    format!("<{}@{}>", uuid::Uuid::new_v4(), domain)
}

/// Who the SMTP server is told the message is from and for (Bcc included).
#[derive(serde::Serialize, serde::Deserialize)]
pub struct SimpleEnvelope {
    pub from: String,
    pub to: Vec<String>,
}

/// A message ready to queue: its bytes and envelope.
pub struct Built {
    pub raw: Vec<u8>,
    pub envelope: SimpleEnvelope,
}

/// Builds the MIME message. Attachments are read now, so a queued message doesn't
/// depend on files that may be gone by the time it is sent.
///
/// A draft keeps its Bcc header (so reopening it shows Bcc) and may have no recipients yet.
pub fn build(acc: &Account, out: &Outgoing, message_id: &str, draft: bool) -> Result<Built> {
    let domain = acc.email.split('@').nth(1).unwrap_or("mailvern.local");
    let from_addr: Address = acc.email.parse()?;
    let from = Mailbox::new(Some(acc.name.clone()).filter(|n| !n.is_empty()), from_addr.clone());
    let mut b = Message::builder().from(from).subject(&out.subject).message_id(Some(message_id.to_string()));
    if draft {
        // Lettre needs an envelope to build; a draft is never sent, so any will do.
        b = b.keep_bcc().envelope(Envelope::new(Some(from_addr.clone()), vec![from_addr])?);
    }
    for t in &out.to {
        b = b.to(mailbox(t)?);
    }
    for t in &out.cc {
        b = b.cc(mailbox(t)?);
    }
    for t in &out.bcc {
        b = b.bcc(mailbox(t)?);
    }
    if let Some(irt) = out.in_reply_to.as_ref().filter(|s| !s.is_empty()) {
        b = b.in_reply_to(irt.clone());
    }
    if let Some(r) = out.references.as_ref().filter(|s| !s.is_empty()) {
        b = b.references(r.clone());
    }

    let (html, images) = extract_inline_images(&out.html, domain);
    let body = if images.is_empty() {
        MultiPart::alternative_plain_html(out.text.clone(), html)
    } else {
        let mut related = MultiPart::related().singlepart(SinglePart::html(html));
        for img in images {
            let ct = ContentType::parse(&img.mime).unwrap_or(ContentType::parse("application/octet-stream").unwrap());
            related = related.singlepart(Attachment::new_inline(img.cid).body(img.bytes, ct));
        }
        MultiPart::alternative().singlepart(SinglePart::plain(out.text.clone())).multipart(related)
    };
    let msg = if out.attachments.is_empty() {
        b.multipart(body)?
    } else {
        let mut mixed = MultiPart::mixed().multipart(body);
        for path in &out.attachments {
            let p = Path::new(path);
            let bytes = std::fs::read(p).with_context(|| format!("Can't read {path}"))?;
            let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "file".into());
            let mime = mime_guess::from_path(p).first_or_octet_stream().to_string();
            let ct = ContentType::parse(&mime).unwrap_or(ContentType::parse("application/octet-stream").unwrap());
            mixed = mixed.singlepart(Attachment::new(name).body(bytes, ct));
        }
        b.multipart(mixed)?
    };

    let env = msg.envelope();
    let envelope = SimpleEnvelope {
        from: env.from().map(|a| a.to_string()).unwrap_or_default(),
        to: env.to().iter().map(|a| a.to_string()).collect(),
    };
    Ok(Built { raw: msg.formatted(), envelope })
}

/// Sends already-built bytes.
pub async fn send_raw(acc: &Account, password: &str, envelope: &SimpleEnvelope, raw: &[u8]) -> Result<()> {
    let from: Address = envelope.from.parse()?;
    let to = envelope.to.iter().map(|a| a.parse::<Address>()).collect::<Result<Vec<_>, _>>()?;
    let env = Envelope::new(Some(from), to)?;
    transport(acc, password)?.send_raw(&env, raw).await.context("Couldn't send")?;
    Ok(())
}

/// Replaces the Date header, so a message queued earlier carries the time it is actually sent.
pub fn redate(raw: &[u8], date: chrono::DateTime<chrono::Local>) -> Vec<u8> {
    let header_end = raw.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 2).unwrap_or(raw.len());
    let new_line = format!("Date: {}\r\n", date.to_rfc2822());
    let mut pos = 0;
    while pos < header_end {
        let line_end = raw[pos..header_end].windows(2).position(|w| w == b"\r\n").map(|p| pos + p + 2).unwrap_or(header_end);
        if raw[pos..line_end].len() >= 5 && raw[pos..pos + 5].eq_ignore_ascii_case(b"date:") {
            // Include folded continuation lines.
            let mut end = line_end;
            while end < header_end && matches!(raw[end], b' ' | b'\t') {
                end = raw[end..header_end].windows(2).position(|w| w == b"\r\n").map(|p| end + p + 2).unwrap_or(header_end);
            }
            let mut out = Vec::with_capacity(raw.len() + 8);
            out.extend_from_slice(&raw[..pos]);
            out.extend_from_slice(new_line.as_bytes());
            out.extend_from_slice(&raw[end..]);
            return out;
        }
        pos = line_end;
    }
    let mut out = new_line.into_bytes();
    out.extend_from_slice(raw);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_images_become_cid_parts() {
        let png = base64::engine::general_purpose::STANDARD.encode(b"\x89PNGfake");
        let html = format!(r#"<p>Hi</p><img src="data:image/png;base64,{png}" style="max-width:100%"><img src='https://x.com/a.png'>"#);
        let (out, imgs) = extract_inline_images(&html, "example.com");
        assert_eq!(imgs.len(), 1);
        assert_eq!(imgs[0].mime, "image/png");
        assert_eq!(imgs[0].bytes, b"\x89PNGfake");
        assert!(out.contains(&format!("src=\"cid:{}\"", imgs[0].cid)));
        assert!(out.contains("https://x.com/a.png"));
        assert!(!out.contains("data:image"));
    }

    fn acc() -> Account {
        Account {
            id: 1,
            name: "Me".into(),
            email: "me@example.com".into(),
            imap_host: String::new(),
            imap_port: 993,
            smtp_host: String::new(),
            smtp_port: 465,
            color: String::new(),
            signature: String::new(),
            position: 0,
            label: String::new(),
        }
    }

    fn out(to: &[&str], bcc: &[&str]) -> Outgoing {
        Outgoing {
            account_id: 1,
            to: to.iter().map(|s| s.to_string()).collect(),
            cc: vec![],
            bcc: bcc.iter().map(|s| s.to_string()).collect(),
            subject: "Hi".into(),
            html: "<p>Hi</p>".into(),
            text: "Hi".into(),
            attachments: vec![],
            in_reply_to: None,
            references: None,
            message_id: None,
        }
    }

    #[test]
    fn sent_mail_hides_bcc_but_envelope_has_it() {
        let b = build(&acc(), &out(&["a@x.com"], &["secret@x.com"]), "<1@example.com>", false).unwrap();
        let raw = String::from_utf8_lossy(&b.raw);
        assert!(!raw.contains("secret@x.com"));
        assert!(raw.contains("Message-ID: <1@example.com>"));
        assert_eq!(b.envelope.to, vec!["a@x.com", "secret@x.com"]);
        assert_eq!(b.envelope.from, "me@example.com");
    }

    #[test]
    fn drafts_keep_bcc_and_need_no_recipients() {
        let b = build(&acc(), &out(&[], &["secret@x.com"]), "<2@example.com>", true).unwrap();
        assert!(String::from_utf8_lossy(&b.raw).contains("Bcc: secret@x.com"));
        build(&acc(), &out(&[], &[]), "<3@example.com>", true).unwrap();
    }

    #[test]
    fn redate_replaces_the_date_header_only() {
        let raw = b"From: a@x.com\r\nDate: Mon, 1 Jan 2024 00:00:00 +0000\r\nSubject: Date: not this\r\n\r\nDate: body".to_vec();
        let when = chrono::Local::now();
        let out = String::from_utf8(redate(&raw, when)).unwrap();
        assert!(out.contains(&format!("Date: {}\r\n", when.to_rfc2822())));
        assert!(!out.contains("2024"));
        assert!(out.contains("Subject: Date: not this"));
        assert!(out.ends_with("\r\n\r\nDate: body"));
    }

    #[test]
    fn html_without_images_is_unchanged() {
        let (out, imgs) = extract_inline_images("<b>plain</b> text about data:image/ in prose", "e.com");
        assert!(imgs.is_empty());
        assert_eq!(out, "<b>plain</b> text about data:image/ in prose");
    }
}
