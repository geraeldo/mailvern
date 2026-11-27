use crate::db::{BodyInfo, NewMessage};
use crate::model::{Addr, AttachmentInfo};
#[cfg(test)]
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
        .map(|ct| ct.c_type.eq_ignore_ascii_case("multipart") && ct.subtype().is_some_and(|s| s.eq_ignore_ascii_case("mixed")))
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
            list_id: None,
            auto: None,
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
        list_id: m.header_raw("List-Id").map(unfold).filter(|s| !s.is_empty()),
        auto: automated(m.header_raw("Auto-Submitted"), m.header_raw("Precedence")),
    }
}

/// A raw header value on one line.
fn unfold(v: &str) -> String {
    v.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether a machine sent the message (RFC 3834), as the header value that says so.
fn automated(auto_submitted: Option<&str>, precedence: Option<&str>) -> Option<String> {
    let value = |v: Option<&str>| v.map(|v| unfold(v).to_lowercase()).filter(|v| !v.is_empty());
    if let Some(a) = value(auto_submitted).filter(|a| a != "no") {
        return Some(a);
    }
    value(precedence).filter(|p| matches!(p.as_str(), "bulk" | "list" | "junk"))
}

fn is_invisible_padding(ch: char) -> bool {
    matches!(ch, '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{FEFF}' | '\u{00AD}' | '\u{034F}')
}

/// Single-pass whitespace normalization and quote-stripping capped at `max_chars`
/// so huge plain-text emails never allocate unbounded intermediate vectors/strings.
fn clean_text_capped(t: &str, max_chars: usize) -> String {
    let mut out = String::with_capacity(t.len().min(max_chars));
    let mut count = 0usize;
    let mut pending_space = false;
    for line in t.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('>') {
            continue;
        }
        for ch in trimmed.chars() {
            if is_invisible_padding(ch) {
                continue;
            }
            if ch.is_whitespace() {
                if count > 0 {
                    pending_space = true;
                }
                continue;
            }
            if pending_space {
                if count >= max_chars {
                    return out;
                }
                out.push(' ');
                count += 1;
                pending_space = false;
            }
            if count >= max_chars {
                return out;
            }
            out.push(ch);
            count += 1;
        }
        if count > 0 {
            pending_space = true;
        }
    }
    out
}

/// Strictly detects whether a `text/plain` part actually contains an HTML document/fragment,
/// avoiding false positives on prose with angle brackets (`<div>`, `Vec<String>`, `<a@b.com>`).
fn looks_like_html(t: &str) -> bool {
    let trimmed = t.trim_start_matches(|c: char| c.is_whitespace() || c == '\u{FEFF}');
    if let Some(after_lt) = trimmed.strip_prefix('<') {
        let after_lt_bytes = after_lt.as_bytes();
        // Leading HTML comment (`<!-- ...`)
        if after_lt_bytes.starts_with(b"!--") {
            return true;
        }
        // Leading document/wrapper tag (`<!doctype`, `<html`, `<head`, `<body`, `<meta`, `<style`, `<table`, `<div`, `<center`)
        let name_end = after_lt_bytes
            .iter()
            .position(|b| !(b.is_ascii_alphanumeric() || *b == b'!'))
            .unwrap_or(after_lt_bytes.len());
        let name = &after_lt[..name_end];
        let next_ok = after_lt_bytes
            .get(name_end)
            .is_none_or(|b| b.is_ascii_whitespace() || matches!(*b, b'>' | b'/'));
        if next_ok
            && (name.eq_ignore_ascii_case("!doctype")
                || name.eq_ignore_ascii_case("html")
                || name.eq_ignore_ascii_case("head")
                || name.eq_ignore_ascii_case("body")
                || name.eq_ignore_ascii_case("meta")
                || name.eq_ignore_ascii_case("style")
                || name.eq_ignore_ascii_case("table")
                || name.eq_ignore_ascii_case("div")
                || name.eq_ignore_ascii_case("center"))
        {
            return true;
        }
    }
    // Check for document-level closing tags in the leading window (avoids scanning huge plain texts).
    let scan_limit = trimmed.floor_char_boundary(trimmed.len().min(8192));
    let head_window = &trimmed[..scan_limit];
    let lower = head_window.to_ascii_lowercase();
    lower.contains("</html>")
        || lower.contains("</body>")
        || lower.contains("</head>")
        || lower.contains("</style>")
}

fn is_skipped_html_tag(tag: &str) -> bool {
    matches!(tag, "head" | "style" | "script" | "title" | "noscript" | "template" | "svg")
}

fn is_block_html_tag(tag: &str) -> bool {
    matches!(
        tag,
        "br" | "p"
            | "div"
            | "tr"
            | "td"
            | "th"
            | "li"
            | "ul"
            | "ol"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "hr"
            | "table"
            | "thead"
            | "tbody"
            | "tfoot"
            | "section"
            | "article"
            | "header"
            | "footer"
            | "nav"
            | "aside"
            | "blockquote"
            | "pre"
            | "dt"
            | "dd"
            | "figcaption"
            | "figure"
            | "address"
            | "main"
            | "form"
            | "option"
    )
}

fn decode_html_entity(entity: &str) -> Option<char> {
    if let Some(num) = entity.strip_prefix('#') {
        let code = if let Some(hex) = num.strip_prefix('x').or_else(|| num.strip_prefix('X')) {
            u32::from_str_radix(hex, 16).ok()?
        } else {
            num.parse::<u32>().ok()?
        };
        return char::from_u32(code);
    }
    match entity {
        "nbsp" | "ensp" | "emsp" | "thinsp" => Some(' '),
        "amp" | "AMP" => Some('&'),
        "lt" | "LT" => Some('<'),
        "gt" | "GT" => Some('>'),
        "quot" | "QUOT" => Some('"'),
        "apos" => Some('\''),
        "ndash" => Some('–'),
        "mdash" => Some('—'),
        "lsquo" => Some('‘'),
        "rsquo" => Some('’'),
        "ldquo" => Some('“'),
        "rdquo" => Some('”'),
        "hellip" => Some('…'),
        "copy" | "COPY" => Some('©'),
        "reg" | "REG" => Some('®'),
        "trade" => Some('™'),
        "bull" => Some('•'),
        "middot" => Some('·'),
        "laquo" => Some('«'),
        "raquo" => Some('»'),
        "zwnj" => Some('\u{200C}'),
        "zwj" => Some('\u{200D}'),
        "shy" => Some('\u{00AD}'),
        _ => None,
    }
}

/// Single-pass, bounded HTML-to-text extractor for snippets and FTS indexing.
/// Skips non-content subtrees (`<head>`, `<style>`, `<script>`, etc.), preserves word
/// boundaries across block/break tags, decodes HTML entities, and strips invisible padding.
fn html_to_clean_text_capped(html: &str, max_chars: usize) -> String {
    let bytes = html.as_bytes();
    let len = bytes.len();
    let mut out = String::with_capacity(len.min(max_chars));
    let mut count = 0usize;
    let mut pending_space = false;
    let mut skip_until: Option<&'static str> = None;
    let mut i = 0usize;

    let push_char = |out: &mut String, count: &mut usize, pending_space: &mut bool, ch: char| -> bool {
        if is_invisible_padding(ch) {
            return true;
        }
        if ch.is_whitespace() {
            if *count > 0 {
                *pending_space = true;
            }
            return true;
        }
        if *pending_space {
            if *count >= max_chars {
                return false;
            }
            out.push(' ');
            *count += 1;
            *pending_space = false;
        }
        if *count >= max_chars {
            return false;
        }
        out.push(ch);
        *count += 1;
        true
    };

    while i < len {
        match bytes[i] {
            b'<' => {
                // HTML comment: <!-- ... -->
                if bytes[i..].starts_with(b"<!--") {
                    if let Some(end) = html[i + 4..].find("-->") {
                        i = i + 4 + end + 3;
                    } else {
                        break;
                    }
                    continue;
                }
                // CDATA: <![CDATA[ ... ]]>
                if bytes[i..].starts_with(b"<![CDATA[") {
                    if let Some(end) = html[i + 9..].find("]]>") {
                        i = i + 9 + end + 3;
                    } else {
                        break;
                    }
                    continue;
                }
                // Declaration / processing instruction: <!DOCTYPE ...> or <?xml ...?>
                if bytes.get(i + 1).is_some_and(|b| matches!(*b, b'!' | b'?')) {
                    if let Some(end) = bytes[i + 2..].iter().position(|&b| b == b'>') {
                        i = i + 2 + end + 1;
                    } else {
                        break;
                    }
                    continue;
                }

                // Regular opening/closing tag.
                let mut j = i + 1;
                while j < len && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                let is_close = j < len && bytes[j] == b'/';
                if is_close {
                    j += 1;
                    while j < len && bytes[j].is_ascii_whitespace() {
                        j += 1;
                    }
                }
                let name_start = j;
                while j < len && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'-' || bytes[j] == b':') {
                    j += 1;
                }
                if j == name_start {
                    // Bare '<' not starting a valid tag (e.g. "a < b").
                    if skip_until.is_none() && !push_char(&mut out, &mut count, &mut pending_space, '<') {
                        return out;
                    }
                    i += 1;
                    continue;
                }

                let mut tag_buf = [0u8; 16];
                let tag_len = (j - name_start).min(tag_buf.len());
                for (dst, &src) in tag_buf[..tag_len].iter_mut().zip(&bytes[name_start..name_start + tag_len]) {
                    *dst = src.to_ascii_lowercase();
                }
                let tag = std::str::from_utf8(&tag_buf[..tag_len]).unwrap_or("");

                // Advance to closing '>', respecting quoted attributes.
                let mut quote: Option<u8> = None;
                let mut end_pos = None;
                let mut k = j;
                while k < len {
                    let b = bytes[k];
                    if let Some(q) = quote {
                        if b == q {
                            quote = None;
                        }
                    } else if b == b'"' || b == b'\'' {
                        quote = Some(b);
                    } else if b == b'>' {
                        end_pos = Some(k);
                        break;
                    }
                    k += 1;
                }
                let Some(gt) = end_pos else {
                    break;
                };
                let self_closing = !is_close && gt > i && bytes[gt - 1] == b'/';

                if let Some(target) = skip_until {
                    if is_close && tag == target {
                        skip_until = None;
                        if count > 0 {
                            pending_space = true;
                        }
                    }
                } else if !is_close && !self_closing && is_skipped_html_tag(tag) {
                    skip_until = match tag {
                        "head" => Some("head"),
                        "style" => Some("style"),
                        "script" => Some("script"),
                        "title" => Some("title"),
                        "noscript" => Some("noscript"),
                        "template" => Some("template"),
                        "svg" => Some("svg"),
                        _ => None,
                    };
                } else if is_block_html_tag(tag) && count > 0 {
                    pending_space = true;
                }

                i = gt + 1;
            }
            b'&' if skip_until.is_none() => {
                let rest = &html[i + 1..];
                let max_entity = rest.len().min(12);
                if let Some(semi) = rest.as_bytes()[..max_entity].iter().position(|&b| b == b';') {
                    let entity = &rest[..semi];
                    if let Some(decoded) = decode_html_entity(entity) {
                        if !push_char(&mut out, &mut count, &mut pending_space, decoded) {
                            return out;
                        }
                        i = i + 1 + semi + 1;
                        continue;
                    }
                }
                if !push_char(&mut out, &mut count, &mut pending_space, '&') {
                    return out;
                }
                i += 1;
            }
            _ => {
                if skip_until.is_some() {
                    // Fast-forward to next '<' while skipping non-content subtrees.
                    if let Some(next_lt) = bytes[i..].iter().position(|&b| b == b'<') {
                        i += next_lt;
                    } else {
                        break;
                    }
                    continue;
                }
                let ch = html[i..].chars().next().unwrap();
                if !push_char(&mut out, &mut count, &mut pending_space, ch) {
                    return out;
                }
                i += ch.len_utf8();
            }
        }
    }
    out
}

/// Custom Tauri URI scheme prefix for inline `cid:` raster images (avoids base64 +33% bloat over IPC).
pub const CID_URI_PREFIX: &str = "mailvern-cid://localhost/";

/// Cap on display HTML after inlining `cid:` images as data URIs, so repeated references
/// to a large attachment can't amplify memory usage unboundedly.
const MAX_DISPLAY_HTML_BYTES: usize = 25 * 1024 * 1024;

fn is_inline_raster_image(mime: &str) -> bool {
    mime.starts_with("image/") && !mime.contains("svg")
}

fn is_cid_boundary_byte(b: u8) -> bool {
    b.is_ascii_whitespace() || matches!(b, b'"' | b'\'' | b')' | b'>' | b'#' | b'?')
}

fn is_cid_boundary(after: &str) -> bool {
    match after.as_bytes().first() {
        None => true,
        Some(&b) => is_cid_boundary_byte(b),
    }
}

fn contains_cid(html: &str, cid: &str) -> bool {
    if cid.is_empty() {
        return false;
    }
    let needle = format!("cid:{cid}");
    let mut rest = html;
    while let Some(pos) = rest.find(&needle) {
        let after = &rest[pos + needle.len()..];
        if is_cid_boundary(after) {
            return true;
        }
        rest = &rest[pos + needle.len()..];
    }
    false
}

/// Replaces every matching `cid:<id>` token in a single linear pass (avoiding O(K²)
/// full-HTML reallocation when an email embeds many inline images).
fn replace_cids(html: &str, map: &std::collections::HashMap<&str, String>, max_len: usize) -> String {
    if map.is_empty() {
        return html.to_string();
    }
    let extra: usize = map.values().map(String::len).sum();
    let mut out = String::with_capacity((html.len().saturating_add(extra)).min(max_len));
    let mut rest = html;
    while let Some(pos) = rest.find("cid:") {
        out.push_str(&rest[..pos]);
        let after_prefix = &rest[pos + 4..];
        let token_len = after_prefix
            .bytes()
            .position(is_cid_boundary_byte)
            .unwrap_or(after_prefix.len());
        let token = &after_prefix[..token_len];
        let after_token = &after_prefix[token_len..];
        if let Some(uri) = map.get(token) {
            if out.len().saturating_add(uri.len()).saturating_add(after_token.len()) <= max_len {
                out.push_str(uri);
            } else {
                out.push_str("cid:");
                out.push_str(token);
            }
        } else {
            out.push_str("cid:");
            out.push_str(token);
        }
        rest = after_token;
    }
    out.push_str(rest);
    out
}

fn listed_attachments<'a>(m: &'a Message<'a>, html: Option<&str>) -> Vec<(usize, &'a mail_parser::MessagePart<'a>)> {
    m.attachments()
        .enumerate()
        .filter(|(_, p)| {
            // Hide inline raster images that the HTML body already shows.
            match (p.content_id(), html) {
                (Some(cid), Some(h)) => {
                    let cid = cid.trim_matches(|c| c == '<' || c == '>').trim();
                    !(is_inline_raster_image(&mime_of(p)) && contains_cid(h, cid))
                }
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
        return BodyInfo { snippet: String::new(), search_text: String::new(), has_attachments: false, attachment_names: String::new() };
    };
    let html = html_body_ref(&m);
    let plain = plain_body_ref(&m);
    let search_text = match (plain, html) {
        (Some(p), _) if !looks_like_html(p) => {
            let cleaned = clean_text_capped(p, 20_000);
            if cleaned.is_empty() {
                html.map(|h| html_to_clean_text_capped(h, 20_000)).unwrap_or_default()
            } else {
                cleaned
            }
        }
        (_, Some(h)) => html_to_clean_text_capped(h, 20_000),
        (Some(p), None) => html_to_clean_text_capped(p, 20_000),
        (None, None) => m.body_text(0).as_deref().map(|t| clean_text_capped(t, 20_000)).unwrap_or_default(),
    };
    let snippet = search_text.chars().take(220).collect();
    let atts = listed_attachments(&m, html);
    let attachment_names = atts
        .iter()
        .filter_map(|(_, p)| p.attachment_name())
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    BodyInfo {
        snippet,
        search_text,
        has_attachments: !atts.is_empty(),
        attachment_names,
    }
}

fn plain_body_ref<'a>(m: &'a Message<'a>) -> Option<&'a str> {
    let part = m.text_part(0)?;
    match &part.body {
        PartType::Text(t) => Some(t.as_ref()),
        _ => None,
    }
}

fn html_body_ref<'a>(m: &'a Message<'a>) -> Option<&'a str> {
    let part = m.html_part(0)?;
    match &part.body {
        PartType::Html(h) => Some(h.as_ref()),
        _ => None,
    }
}

pub struct ParsedBody {
    pub bcc: Vec<Addr>,
    pub html: Option<String>,
    pub text: Option<String>,
    pub attachments: Vec<AttachmentInfo>,
    pub warnings: Vec<crate::model::SecurityWarning>,
    pub calendar_invite: Option<crate::model::CalendarInvite>,
}

/// An inline raster image part extracted from a message for serving over `mailvern-cid://`.
pub type InlineCidPart = (usize, String, Vec<u8>);

fn parsed_body_common(m: &Message<'_>, original_html: Option<&str>, html: Option<String>) -> ParsedBody {
    let text = if html.is_none() { m.body_text(0).map(|t| t.to_string()) } else { None };
    let attachments = listed_attachments(m, original_html)
        .into_iter()
        .map(|(i, p)| AttachmentInfo {
            index: i,
            filename: attachment_name(p, i),
            mime: mime_of(p),
            size: p.contents().len(),
        })
        .collect();
    let from = addrs(m.from()).into_iter().next();
    let from_name = from.as_ref().and_then(|f| f.name.as_deref()).unwrap_or("");
    let from_addr = from.as_ref().map(|f| f.email.as_str()).unwrap_or("");
    let reply_to = addrs(m.reply_to());
    let list_id = m.header_raw("List-Id").map(unfold).filter(|s| !s.is_empty());
    let warnings = crate::security::analyze_headers(m, from_name, from_addr, &reply_to, list_id.as_deref());
    let calendar_invite = crate::ics::parse_from_message(m);
    ParsedBody { bcc: addrs(m.bcc()), html, text, attachments, warnings, calendar_invite }
}

/// Prepares a stored message for display using `mailvern-cid://localhost/{message_id}/{part_index}`
/// URLs for inline raster images instead of base64 `data:` URIs. Returns the parsed body plus the
/// extracted inline image bytes so the engine can populate its in-memory URI cache without re-parsing.
pub fn display_for_message(message_id: i64, raw: &[u8]) -> (ParsedBody, Vec<InlineCidPart>) {
    let Some(m) = MessageParser::default().parse(raw) else {
        return (
            ParsedBody { bcc: vec![], html: None, text: Some(String::from_utf8_lossy(raw).into()), attachments: vec![], warnings: vec![], calendar_invite: None },
            vec![],
        );
    };
    let original_html = html_body_ref(&m);
    let mut inline_parts: Vec<InlineCidPart> = Vec::new();
    let html = original_html.map(|h| {
        let mut cid_map: std::collections::HashMap<&str, String> = std::collections::HashMap::new();
        for (part_idx, p) in m.attachments().enumerate() {
            let Some(cid) = p.content_id() else { continue };
            let cid = cid.trim_matches(|c| c == '<' || c == '>').trim();
            if cid.is_empty() || cid_map.contains_key(cid) {
                continue;
            }
            let mime = mime_of(p);
            if !is_inline_raster_image(&mime) || !contains_cid(h, cid) {
                continue;
            }
            let uri = format!("{CID_URI_PREFIX}{message_id}/{part_idx}");
            cid_map.insert(cid, uri);
            inline_parts.push((part_idx, mime, p.contents().to_vec()));
        }
        replace_cids(h, &cid_map, MAX_DISPLAY_HTML_BYTES)
    });
    (parsed_body_common(&m, original_html, html), inline_parts)
}

/// Extracts a single inline raster image by attachment index for serving via `mailvern-cid://`.
/// Strictly rejects non-raster types (including SVGs) to match display security guarantees.
pub fn inline_cid_image(raw: &[u8], part_index: usize) -> Option<(String, Vec<u8>)> {
    let m = MessageParser::default().parse(raw)?;
    let p = m.attachments().nth(part_index)?;
    let mime = mime_of(p);
    if !is_inline_raster_image(&mime) {
        return None;
    }
    Some((mime, p.contents().to_vec()))
}

/// Prepares a stored message for display. Inline `cid:` images become data URIs in one linear pass.
#[cfg(test)]
pub fn display(raw: &[u8]) -> ParsedBody {
    let Some(m) = MessageParser::default().parse(raw) else {
        return ParsedBody { bcc: vec![], html: None, text: Some(String::from_utf8_lossy(raw).into()), attachments: vec![], warnings: vec![], calendar_invite: None };
    };
    let original_html = html_body_ref(&m);
    let html = original_html.map(|h| {
        let mut cid_map: std::collections::HashMap<&str, String> = std::collections::HashMap::new();
        let mut budget = h.len();
        for p in m.attachments() {
            let Some(cid) = p.content_id() else { continue };
            let cid = cid.trim_matches(|c| c == '<' || c == '>').trim();
            if cid.is_empty() || cid_map.contains_key(cid) {
                continue;
            }
            let mime = mime_of(p);
            if !is_inline_raster_image(&mime) || !contains_cid(h, cid) {
                continue;
            }
            // Skip encoding if even one copy would exceed our cap.
            let encoded_len = p.contents().len().div_ceil(3).saturating_mul(4);
            if budget.saturating_add(encoded_len) > MAX_DISPLAY_HTML_BYTES {
                continue;
            }
            let data = base64::engine::general_purpose::STANDARD.encode(p.contents());
            let uri = format!("data:{mime};base64,{data}");
            budget = budget.saturating_add(uri.len());
            cid_map.insert(cid, uri);
        }
        replace_cids(h, &cid_map, MAX_DISPLAY_HTML_BYTES)
    });
    parsed_body_common(&m, original_html, html)
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

/// Returns (filename, mime type, bytes) for attachment `index` (as numbered by `display`).
pub fn attachment_part(raw: &[u8], index: usize) -> Option<(String, String, Vec<u8>)> {
    let m = MessageParser::default().parse(raw)?;
    let p = m.attachments().nth(index)?;
    let (mime, bytes) = match &p.body {
        PartType::Message(inner) => ("message/rfc822".to_string(), inner.raw_message().to_vec()),
        _ => (mime_of(p), p.contents().to_vec()),
    };
    Some((attachment_name(p, index), mime, bytes))
}

/// Every listed attachment as (filename, bytes), for forwarding.
pub fn all_attachments(raw: &[u8]) -> Vec<(String, Vec<u8>)> {
    let Some(m) = MessageParser::default().parse(raw) else {
        return vec![];
    };
    let html = html_body_ref(&m);
    listed_attachments(&m, html)
        .into_iter()
        .map(|(i, p)| {
            let bytes = match &p.body {
                PartType::Message(inner) => inner.raw_message().to_vec(),
                _ => p.contents().to_vec(),
            };
            (attachment_name(p, i), bytes)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_for_rules() {
        let raw = b"From: Hub <n@hub.example>\r\nList-Id: repo <repo.hub.example>\r\nPrecedence: bulk\r\nSubject: x\r\n\r\nbody";
        let m = parse_header(1, raw, false, false, 0, 0);
        assert_eq!(m.list_id.as_deref(), Some("repo <repo.hub.example>"));
        assert_eq!(m.auto.as_deref(), Some("bulk"));
        let raw = b"From: a@x.com\r\nAuto-Submitted: no\r\nPrecedence: first-class\r\nSubject: x\r\n\r\nbody";
        let m = parse_header(1, raw, false, false, 0, 0);
        assert_eq!((m.list_id, m.auto), (None, None));
        let raw = b"From: a@x.com\r\nAuto-Submitted: Auto-Replied\r\nSubject: x\r\n\r\nbody";
        assert_eq!(parse_header(1, raw, false, false, 0, 0).auto.as_deref(), Some("auto-replied"));
    }

    #[test]
    fn cid_inlining_respects_boundaries_and_raster_types() {
        let raw = concat!(
            "From: a@x.com\r\n",
            "Subject: cid\r\n",
            "MIME-Version: 1.0\r\n",
            "Content-Type: multipart/related; boundary=\"b\"\r\n\r\n",
            "--b\r\n",
            "Content-Type: text/html; charset=utf-8\r\n\r\n",
            "<img src=\"cid:img1\"><img src=\"cid:img10\"><img src=\"cid:vec\">\r\n",
            "--b\r\n",
            "Content-Type: image/png\r\n",
            "Content-ID: <img1>\r\n",
            "Content-Transfer-Encoding: base64\r\n\r\n",
            "AQID\r\n",
            "--b\r\n",
            "Content-Type: image/png\r\n",
            "Content-ID: <img10>\r\n",
            "Content-Transfer-Encoding: base64\r\n\r\n",
            "BAUG\r\n",
            "--b\r\n",
            "Content-Type: image/svg+xml; name=\"x.svg\"\r\n",
            "Content-ID: <vec>\r\n",
            "Content-Disposition: inline; filename=\"x.svg\"\r\n\r\n",
            "<svg></svg>\r\n",
            "--b\r\n",
            "Content-Type: application/pdf; name=\"doc.pdf\"\r\n",
            "Content-Disposition: attachment; filename=\"doc.pdf\"\r\n\r\n",
            "%PDF\r\n",
            "--b--\r\n",
        )
        .as_bytes();
        let d = display(raw);
        let html = d.html.expect("html");
        assert!(html.contains("src=\"data:image/png;base64,AQID\""));
        assert!(html.contains("src=\"data:image/png;base64,BAUG\""));
        // SVG cid is NOT inlined as a data URI, and remains listed as an attachment.
        assert!(html.contains("src=\"cid:vec\""));
        let names: Vec<&str> = d.attachments.iter().map(|a| a.filename.as_str()).collect();
        assert_eq!(names, vec!["x.svg", "doc.pdf"]);
        let all = all_attachments(raw);
        assert_eq!(all.len(), 2);
        assert_eq!(all[1].0, "doc.pdf");
        assert_eq!(all[1].1, b"%PDF");

        // URI scheme mode rewrites inline raster images to mailvern-cid:// without base64 bloat,
        // extracts the raw parts, and still refuses SVGs.
        let (d2, parts) = display_for_message(77, raw);
        let html2 = d2.html.expect("html2");
        assert!(html2.contains("src=\"mailvern-cid://localhost/77/0\""));
        assert!(html2.contains("src=\"mailvern-cid://localhost/77/1\""));
        assert!(html2.contains("src=\"cid:vec\""));
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], (0, "image/png".to_string(), vec![1, 2, 3]));
        assert_eq!(parts[1], (1, "image/png".to_string(), vec![4, 5, 6]));
        assert_eq!(inline_cid_image(raw, 0), Some(("image/png".to_string(), vec![1, 2, 3])));
        assert_eq!(inline_cid_image(raw, 2), None); // SVG refused
    }

    #[test]
    fn snippet_strips_html_in_text_plain_and_preserves_block_spacing() {
        // Broken sender puts full HTML document inside text/plain.
        let html_in_plain = concat!(
            "From: Redhub <redhub@mail.redcomm.co.id>\r\n",
            "Subject: Reminder\r\n",
            "Content-Type: text/plain; charset=utf-8\r\n\r\n",
            "<!DOCTYPE html><html lang=\"en\" xmlns=\"http://www.w3.org/1999/xhtml\">",
            "<head><title>Reminder</title><style>.btn { color: red; }</style></head>",
            "<body><!-- header --><div><p>Dear Semangat!,</p>",
            "<p>Hari ini merupakan jadwal pembayaran terakhir &amp; upload dokumen.</p>",
            "</div></body></html>",
        )
        .as_bytes();
        let info = body_info(html_in_plain);
        assert_eq!(
            info.snippet,
            "Dear Semangat!, Hari ini merupakan jadwal pembayaran terakhir & upload dokumen."
        );
        assert!(!info.search_text.contains("DOCTYPE"));
        assert!(!info.search_text.contains("color: red"));

        // HTML-only email preserves spaces across block elements & strips invisible preheader padding.
        let html_only = concat!(
            "From: Anthropic <no-reply@anthropic.com>\r\n",
            "Subject: Sign in\r\n",
            "Content-Type: text/html; charset=utf-8\r\n\r\n",
            "<html><head><style>body{margin:0}</style></head><body>",
            "<h1>Sign in to Claude</h1>",
            "<p>Click the button below to continue&zwnj;&nbsp;&zwnj;.</p>",
            "</body></html>",
        )
        .as_bytes();
        let info = body_info(html_only);
        assert_eq!(info.snippet, "Sign in to Claude Click the button below to continue .");

        // Genuine text/plain with intentional HTML tags, generics, and angle-bracket addresses stays untouched.
        let code_plain = concat!(
            "From: Dev <dev@example.com>\r\n",
            "Subject: Code review\r\n",
            "Content-Type: text/plain; charset=utf-8\r\n\r\n",
            "Can you change <div class=\"card\"> to <section> and return Vec<String>?\r\n",
            "cc: Alice <alice@example.com>\r\n",
        )
        .as_bytes();
        let info = body_info(code_plain);
        assert_eq!(
            info.snippet,
            "Can you change <div class=\"card\"> to <section> and return Vec<String>? cc: Alice <alice@example.com>"
        );
    }
}
