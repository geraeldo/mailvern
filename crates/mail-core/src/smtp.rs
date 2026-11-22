use crate::model::{Account, Outgoing};
use anyhow::{anyhow, Context, Result};
use lettre::message::header::ContentType;
use lettre::message::{Attachment, Mailbox, MultiPart};
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

    let body = MultiPart::alternative_plain_html(out.text.clone(), out.html.clone());
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
