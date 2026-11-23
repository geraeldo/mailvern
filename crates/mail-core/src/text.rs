//! Mail bodies as plain text for a language model, and received HTML made safe to quote.
//! The Rust twin of `src/lib/ai/text.ts`, for callers outside the webview.

use crate::extract::{html_plain, tidy};

/// The readable body of a message: its HTML as text (with link addresses and image
/// descriptions), or its plain text. `strip_quotes` leaves out quoted earlier messages.
pub fn body_text(html: Option<&str>, text: Option<&str>, strip_quotes: bool) -> String {
    match html {
        Some(h) if strip_quotes => html_plain(&strip_quoted_html(h), true),
        Some(h) => html_plain(h, true),
        None => {
            let t = tidy(text.unwrap_or(""));
            if strip_quotes { strip_plain_quotes(&t) } else { t }
        }
    }
}

/// HTML as plain text, without link addresses.
pub fn html_to_text(html: &str) -> String {
    html_plain(html, false)
}

pub fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// Where the element that was opened just before `pos` ends (after its closing tag).
/// `lower` is the lowercased document; nested elements of the same name are skipped over.
fn element_end(lower: &str, mut pos: usize, name: &str) -> usize {
    let (open, close) = (format!("<{name}"), format!("</{name}"));
    let is_tag = |at: usize| !lower[at..].starts_with(|c: char| c.is_ascii_alphanumeric());
    let mut depth = 1;
    loop {
        let Some(c) = lower[pos..].find(&close).map(|i| pos + i) else { return lower.len() };
        match lower[pos..c].find(&open).map(|i| pos + i + open.len()) {
            Some(after) => {
                if is_tag(after) {
                    depth += 1;
                }
                pos = after;
            }
            None => {
                pos = lower[c..].find('>').map_or(lower.len(), |i| c + i + 1);
                if is_tag(c + close.len()) {
                    depth -= 1;
                    if depth == 0 {
                        return pos;
                    }
                }
            }
        }
    }
}

/// Removes quoted earlier messages from HTML: Gmail and Apple Mail quote blocks, and
/// everything below Outlook's reply header (Outlook puts the old message under it).
pub fn strip_quoted_html(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut pos = 0;
    while let Some(lt) = lower[pos..].find('<').map(|i| pos + i) {
        let Some(gt) = lower[lt..].find('>').map(|i| lt + i) else { break };
        let tag = &lower[lt + 1..gt];
        let name = tag.split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or("");
        let has = |s: &str| tag.contains(s);
        if has("id=\"appendonsend\"") || has("id=\"divrplyfwdmsg\"") {
            out.push_str(&html[pos..lt]);
            return out;
        }
        let cite = name == "blockquote" && (has("type=\"cite\"") || has("type='cite'") || has("type=cite"));
        if !name.is_empty() && (cite || has("gmail_quote") || has("gmail_extra") || has("mv-quote-head")) {
            out.push_str(&html[pos..lt]);
            pos = element_end(&lower, gt + 1, name);
            continue;
        }
        out.push_str(&html[pos..=gt]);
        pos = gt + 1;
    }
    out.push_str(&html[pos..]);
    out
}

/// Drops quoted lines ("> …") and the "On … wrote:" line before them from plain text.
pub fn strip_plain_quotes(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let quoted = |i: usize| lines.get(i).is_some_and(|l| l.trim_start().starts_with('>'));
    let mut keep = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if quoted(i) {
            continue;
        }
        let end = l.trim_end().to_lowercase();
        let intro = end.ends_with("wrote:") || end.ends_with("menulis:");
        if intro && (quoted(i + 1) || (lines.get(i + 1).is_some_and(|n| n.trim().is_empty()) && quoted(i + 2))) {
            continue;
        }
        keep.push(*l);
    }
    tidy(&keep.join("\n"))
}

/// Received HTML made fit to sit inside another message as a quote: no document shell,
/// head, scripts, styles or comments, so it can't restyle the reply around it.
/// The composer sanitizes it fully when the draft is opened.
pub fn quotable_html(html: &str) -> String {
    const SKIP: &[&str] = &["head", "script", "style", "title"];
    const DROP: &[&str] = &["html", "body", "meta", "link", "base", "!doctype"];
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut pos = 0;
    while let Some(lt) = lower[pos..].find('<').map(|i| pos + i) {
        out.push_str(&html[pos..lt]);
        if lower[lt..].starts_with("<!--") {
            pos = lower[lt..].find("-->").map_or(lower.len(), |i| lt + i + 3);
            continue;
        }
        let Some(gt) = lower[lt..].find('>').map(|i| lt + i) else {
            pos = lt;
            break;
        };
        let tag = &lower[lt + 1..gt];
        let closing = tag.starts_with('/');
        let name = tag.trim_start_matches('/').split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or("");
        pos = gt + 1;
        if SKIP.contains(&name) && !closing {
            pos = element_end(&lower, pos, name);
        } else if !DROP.contains(&name) && !SKIP.contains(&name) {
            out.push_str(&html[lt..=gt]);
        }
    }
    out.push_str(&html[pos..]);
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_are_removed_from_html() {
        let gmail = r#"<div>Sounds good.</div><div class="gmail_quote"><div>On Mon, Budi wrote:</div><blockquote class="gmail_quote"><div>Old <div>nested</div></div></blockquote></div><div>PS: thanks</div>"#;
        assert_eq!(body_text(Some(gmail), None, true), "Sounds good.\n\nPS: thanks");
        let apple = r#"<div>Yes.</div><BLOCKQUOTE type="cite"><blockquote type="cite">older</blockquote>old</BLOCKQUOTE><div>Inline answer</div>"#;
        assert_eq!(body_text(Some(apple), None, true), "Yes.\n\nInline answer");
        let outlook = r#"<p>Agreed.</p><div id="appendonsend"></div><hr><div id="divRplyFwdMsg">From: x</div><div>old body</div>"#;
        assert_eq!(body_text(Some(outlook), None, true), "Agreed.");
        assert!(body_text(Some(gmail), None, false).contains("nested"));
        // An unclosed quote takes the rest of the message with it, as a browser would.
        assert_eq!(body_text(Some("<p>Hi</p><blockquote type=cite>old"), None, true), "Hi");
    }

    #[test]
    fn quotes_are_removed_from_plain_text() {
        let t = "Sure, Friday works.\n\nOn Mon, 3 Nov 2026, Budi <b@x.co> wrote:\n> Can we meet?\n> Friday?\n\nSent from my phone";
        assert_eq!(body_text(None, Some(t), true), "Sure, Friday works.\n\nSent from my phone");
        assert_eq!(strip_plain_quotes("Budi wrote: hello\nnot a quote"), "Budi wrote: hello\nnot a quote");
    }

    #[test]
    fn quotable_html_drops_the_document_shell() {
        let html = "<!DOCTYPE html><html><head><style>p{color:red}</style><title>t</title></head><body class=\"x\"><!-- c --><p>Hi <b>there</b></p><script>alert(1)</script><style>a{}</style></body></html>";
        assert_eq!(quotable_html(html), "<p>Hi <b>there</b></p>");
        assert_eq!(quotable_html("plain &amp; simple"), "plain &amp; simple");
        assert_eq!(escape_html("<a href=\"x\">&'"), "&lt;a href=&quot;x&quot;&gt;&amp;&#39;");
    }
}
