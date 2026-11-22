use crate::model::{Account, Outgoing};
use anyhow::{anyhow, Context, Result};
use lettre::message::header::ContentType;
use base64::Engine as _;
use lettre::message::{Attachment, Mailbox, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
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

/// Builds and sends the message. Returns (Message-ID, raw bytes).
pub async fn send(acc: &Account, password: &str, out: &Outgoing) -> Result<(String, Vec<u8>)> {
    let domain = acc.email.split('@').nth(1).unwrap_or("mailvern.local");
    let message_id = format!("<{}@{}>", uuid::Uuid::new_v4(), domain);

    let from = Mailbox::new(Some(acc.name.clone()).filter(|n| !n.is_empty()), acc.email.parse()?);
    let mut b = Message::builder().from(from).subject(&out.subject).message_id(Some(message_id.clone()));
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

    let raw = msg.formatted();
    transport(acc, password)?.send(msg).await.context("Couldn't send")?;
    Ok((message_id, raw))
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

    #[test]
    fn html_without_images_is_unchanged() {
        let (out, imgs) = extract_inline_images("<b>plain</b> text about data:image/ in prose", "e.com");
        assert!(imgs.is_empty());
        assert_eq!(out, "<b>plain</b> text about data:image/ in prose");
    }
}
