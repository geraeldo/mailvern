use crate::db::{BodyInfo, NewMessage};
use crate::model::{Addr, AttachmentInfo};
use base64::Engine as _;
use mail_parser::{Address, HeaderValue, Message, MessageParser, MimeHeaders, PartType};

fn addrs(a: Option<&Address>) -> Vec<Addr> {
    let list: Vec<&mail_parser::Addr> = match a {
        None => vec![],
        Some(Address::List(v)) => v.iter().collect(),
        Some(Address::Group(gs)) => gs.iter().flat_map(|g| g.addresses.iter()).collect(),
    };
    list.into_iter()
        .filter_map(|x| {
            x.address.as_ref().map(|e| Addr {
                name: x.name.as_ref().map(|n| n.trim().to_string()).filter(|n| !n.is_empty()),
                email: e.to_string(),
            })
        })
        .collect()
}

fn id_list(v: &HeaderValue) -> Option<String> {
    let ids: Vec<String> = match v {
        HeaderValue::Text(t) => vec![t.to_string()],
        HeaderValue::TextList(l) => l.iter().map(|t| t.to_string()).collect(),
        _ => vec![],
    };
    if ids.is_empty() {
        None
    } else {
        Some(ids.iter().map(|i| format!("<{}>", i)).collect::<Vec<_>>().join(" "))
    }
}

fn is_multipart_mixed(m: &Message) -> bool {
    m.root_part()
        .content_type()
        .map(|ct| ct.c_type.eq_ignore_ascii_case("multipart") && ct.subtype().map_or(false, |s| s.eq_ignore_ascii_case("mixed")))
        .unwrap_or(false)
}

/// Parses the header block fetched during sync.
pub fn parse_header(uid: u32, raw: &[u8], seen: bool, flagged: bool, size: u32, internal_date: i64) -> NewMessage {
    let parsed = MessageParser::default().parse(raw);
    let Some(m) = parsed else {
        return NewMessage {
            uid,
            message_id: None,
            in_reply_to: None,
            refs: None,
            subject: String::new(),
            from_name: String::new(),
            from_addr: String::new(),
            to: vec![],
            cc: vec![],
            reply_to: vec![],
            date: internal_date,
            seen,
            flagged,
            has_attachments: false,
            size,
        };
    };
    let from = addrs(m.from()).into_iter().next();
    let date = m.date().map(|d| d.to_timestamp()).filter(|d| *d > 0).unwrap_or(internal_date);
    NewMessage {
        uid,
        message_id: m.message_id().map(|s| format!("<{}>", s)),
        in_reply_to: id_list(m.in_reply_to()),
        refs: id_list(m.references()),
        subject: m.subject().unwrap_or("").trim().to_string(),
        from_name: from.as_ref().and_then(|f| f.name.clone()).unwrap_or_default(),
        from_addr: from.map(|f| f.email).unwrap_or_default(),
        to: addrs(m.to()),
        cc: addrs(m.cc()),
        reply_to: addrs(m.reply_to()),
        date,
        seen,
        flagged,
        has_attachments: is_multipart_mixed(&m),
        size,
    }
}

fn clean_text(t: &str) -> String {
    t.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('>'))
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn listed_attachments<'a>(m: &'a Message<'a>, html: Option<&str>) -> Vec<(usize, &'a mail_parser::MessagePart<'a>)> {
    m.attachments()
        .enumerate()
        .filter(|(_, p)| {
            // Hide inline images that the HTML body already shows.
            match (p.content_id(), html) {
                (Some(cid), Some(h)) => !h.contains(&format!("cid:{}", cid.trim_matches(|c| c == '<' || c == '>'))),
                _ => true,
            }
        })
        .collect()
}

fn attachment_name(p: &mail_parser::MessagePart, i: usize) -> String {
    p.attachment_name()
        .map(|s| s.to_string())
        .or_else(|| match &p.body {
            PartType::Message(inner) => inner.subject().map(|s| format!("{}.eml", s)),
            _ => None,
        })
        .unwrap_or_else(|| format!("attachment-{}", i + 1))
}

fn mime_of(p: &mail_parser::MessagePart) -> String {
    p.content_type()
        .map(|ct| match ct.subtype() {
            Some(s) => format!("{}/{}", ct.c_type, s),
            None => ct.c_type.to_string(),
        })
        .unwrap_or_else(|| "application/octet-stream".into())
        .to_lowercase()
}

/// Computes list/search fields after a full body download.
pub fn body_info(raw: &[u8]) -> BodyInfo {
    let Some(m) = MessageParser::default().parse(raw) else {
        return BodyInfo { snippet: String::new(), search_text: String::new(), has_attachments: false };
    };
    let text = m.body_text(0).map(|t| t.to_string()).unwrap_or_default();
    let cleaned = clean_text(&text);
    let html = html_body(&m);
    BodyInfo {
        snippet: cleaned.chars().take(220).collect(),
        search_text: cleaned.chars().take(20_000).collect(),
        has_attachments: !listed_attachments(&m, html.as_deref()).is_empty(),
    }
}

fn html_body(m: &Message) -> Option<String> {
    let part = m.html_part(0)?;
    match &part.body {
        PartType::Html(h) => Some(h.to_string()),
        _ => None,
    }
}

pub struct ParsedBody {
    pub bcc: Vec<Addr>,
    pub html: Option<String>,
    pub text: Option<String>,
    pub attachments: Vec<AttachmentInfo>,
}

/// Prepares a stored message for display. Inline `cid:` images become data URIs.
pub fn display(raw: &[u8]) -> ParsedBody {
    let Some(m) = MessageParser::default().parse(raw) else {
        return ParsedBody { bcc: vec![], html: None, text: Some(String::from_utf8_lossy(raw).into()), attachments: vec![] };
    };
    let mut html = html_body(&m);
    if let Some(h) = html.as_mut() {
        for p in m.attachments() {
            if let Some(cid) = p.content_id() {
                let cid = cid.trim_matches(|c| c == '<' || c == '>');
                let needle = format!("cid:{}", cid);
                if h.contains(&needle) {
                    let data = base64::engine::general_purpose::STANDARD.encode(p.contents());
                    *h = h.replace(&needle, &format!("data:{};base64,{}", mime_of(p), data));
                }
            }
        }
    }
    let text = if html.is_none() { m.body_text(0).map(|t| t.to_string()) } else { None };
    let original_html = html_body(&m);
    let attachments = listed_attachments(&m, original_html.as_deref())
        .into_iter()
        .map(|(i, p)| AttachmentInfo {
            index: i,
            filename: attachment_name(p, i),
            mime: mime_of(p),
            size: p.contents().len(),
        })
        .collect();
    ParsedBody { bcc: addrs(m.bcc()), html, text, attachments }
}

/// Returns (filename, bytes) for attachment `index` (as numbered by `display`).
pub fn attachment(raw: &[u8], index: usize) -> Option<(String, Vec<u8>)> {
    let m = MessageParser::default().parse(raw)?;
    let p = m.attachments().nth(index)?;
    let bytes = match &p.body {
        PartType::Message(inner) => inner.raw_message().to_vec(),
        _ => p.contents().to_vec(),
    };
    Some((attachment_name(p, index), bytes))
}

/// Every listed attachment as (filename, bytes), for forwarding.
pub fn all_attachments(raw: &[u8]) -> Vec<(String, Vec<u8>)> {
    display(raw)
        .attachments
        .iter()
        .filter_map(|a| attachment(raw, a.index))
        .collect()
}
