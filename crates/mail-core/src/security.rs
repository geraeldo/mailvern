//! Header-level phishing and spoofing analysis (DMARC/SPF/DKIM failures,
//! deceptive display names, BEC Reply-To mismatches, and homograph domains).

use crate::model::{Addr, SecurityWarning};
use mail_parser::Message;

const FREE_PROVIDERS: &[&str] = &[
    "gmail.com",
    "googlemail.com",
    "outlook.com",
    "hotmail.com",
    "live.com",
    "msn.com",
    "yahoo.com",
    "ymail.com",
    "icloud.com",
    "me.com",
    "mac.com",
    "aol.com",
    "proton.me",
    "protonmail.com",
    "zoho.com",
    "mail.ru",
    "yandex.com",
    "yandex.ru",
    "gmx.com",
    "gmx.de",
];

fn email_domain(email: &str) -> Option<&str> {
    let (_, dom) = email.trim().rsplit_once('@')?;
    let dom = dom.trim().trim_end_matches('.');
    (!dom.is_empty()).then_some(dom)
}

fn registrable_domain(domain: &str) -> String {
    let lower = domain.to_ascii_lowercase();
    psl::domain_str(&lower).unwrap_or(&lower).to_string()
}

fn is_free_provider(domain: &str) -> bool {
    let reg = registrable_domain(domain);
    FREE_PROVIDERS.iter().any(|p| *p == reg)
}

/// Extracts an email address embedded inside a display name, e.g. `"ceo@acme.com"`.
fn embedded_email(display_name: &str) -> Option<String> {
    for tok in display_name.split(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '(' | ')' | '"' | '\'' | ',' | ';')) {
        let t = tok.trim();
        if let Some((local, dom)) = t.split_once('@') {
            if !local.is_empty() && dom.contains('.') && !dom.starts_with('.') && !dom.ends_with('.') {
                return Some(t.to_ascii_lowercase());
            }
        }
    }
    None
}

fn has_mixed_confusable_script(s: &str) -> bool {
    let mut has_latin = false;
    let mut has_cyrillic_or_greek = false;
    for ch in s.chars() {
        let cp = ch as u32;
        if ch.is_ascii_alphabetic() {
            has_latin = true;
        } else if (0x0400..=0x04FF).contains(&cp) || (0x0370..=0x03FF).contains(&cp) {
            has_cyrillic_or_greek = true;
        }
    }
    has_latin && has_cyrillic_or_greek
}

/// Inspects raw RFC 822 headers for high-signal spoofing/phishing indicators.
pub fn analyze_headers(
    m: &Message<'_>,
    from_name: &str,
    from_addr: &str,
    reply_to: &[Addr],
    list_id: Option<&str>,
) -> Vec<SecurityWarning> {
    let mut warnings = Vec::new();
    let from_lc = from_addr.trim().to_ascii_lowercase();
    let from_dom = email_domain(&from_lc).map(str::to_string);

    // 1. DMARC / SPF+DKIM authentication failure from Authentication-Results headers.
    let mut dmarc_fail = false;
    let mut spf_fail = false;
    let mut dkim_fail = false;
    let mut dkim_pass = false;
    for h in m.headers() {
        let name = h.name();
        if name.eq_ignore_ascii_case("Authentication-Results") || name.eq_ignore_ascii_case("ARC-Authentication-Results") {
            if let Some(raw) = h.value().as_text() {
                let lc = raw.to_ascii_lowercase();
                if lc.contains("dmarc=fail") {
                    dmarc_fail = true;
                }
                if lc.contains("spf=fail") || lc.contains("spf=hardfail") {
                    spf_fail = true;
                }
                if lc.contains("dkim=fail") {
                    dkim_fail = true;
                }
                if lc.contains("dkim=pass") {
                    dkim_pass = true;
                }
            }
        } else if name.eq_ignore_ascii_case("Received-SPF") {
            if let Some(raw) = h.value().as_text() {
                let lc = raw.trim().to_ascii_lowercase();
                if lc.starts_with("fail") {
                    spf_fail = true;
                }
            }
        }
    }
    if dmarc_fail || (spf_fail && dkim_fail && !dkim_pass) {
        warnings.push(SecurityWarning {
            kind: "auth_fail".into(),
            severity: "danger".into(),
            title: "Sender couldn't be verified".into(),
            detail: "This message failed email authentication (DMARC/SPF/DKIM) and may be spoofed.".into(),
        });
    }

    // 2. Display name impersonating another email address.
    if !from_name.trim().is_empty() && !from_lc.is_empty() {
        if let Some(spoofed) = embedded_email(from_name) {
            if spoofed != from_lc {
                warnings.push(SecurityWarning {
                    kind: "display_name_spoof".into(),
                    severity: "danger".into(),
                    title: "Deceptive sender name".into(),
                    detail: format!(
                        "The sender name shows “{spoofed}”, but this email actually came from {from_lc}."
                    ),
                });
            }
        }
    }

    // 3. Punycode or mixed-script homograph domain in From.
    if let Some(dom) = &from_dom {
        if dom.contains("xn--") || has_mixed_confusable_script(dom) {
            warnings.push(SecurityWarning {
                kind: "homograph_domain".into(),
                severity: "warn".into(),
                title: "Unusual sender domain".into(),
                detail: format!("The sender domain ({dom}) uses internationalized or lookalike characters."),
            });
        }
    }

    // 4. BEC-style Reply-To mismatch: custom org domain From -> free consumer webmail Reply-To.
    if list_id.is_none() {
        if let (Some(fdom), Some(rt)) = (&from_dom, reply_to.first()) {
            if let Some(rdom) = email_domain(&rt.email) {
                let f_reg = registrable_domain(fdom);
                let r_reg = registrable_domain(rdom);
                if f_reg != r_reg && !is_free_provider(fdom) && is_free_provider(rdom) {
                    warnings.push(SecurityWarning {
                        kind: "reply_to_mismatch".into(),
                        severity: "warn".into(),
                        title: "Replies go to a different address".into(),
                        detail: format!(
                            "This email appears from {from_lc}, but replies will be sent to {}.",
                            rt.email
                        ),
                    });
                }
            }
        }
    }

    warnings
}

#[cfg(test)]
mod tests {
    use super::*;
    use mail_parser::MessageParser;

    #[test]
    fn flags_dmarc_fail_and_display_name_spoof() {
        let raw = concat!(
            "From: \"billing@paypal.com\" <scam@evil.example>\r\n",
            "To: me@example.com\r\n",
            "Authentication-Results: mx.example.com; dmarc=fail spf=fail\r\n",
            "Subject: Urgent\r\n\r\nHi"
        );
        let m = MessageParser::default().parse(raw.as_bytes()).unwrap();
        let w = analyze_headers(&m, "billing@paypal.com", "scam@evil.example", &[], None);
        assert!(w.iter().any(|x| x.kind == "auth_fail"));
        assert!(w.iter().any(|x| x.kind == "display_name_spoof"));
    }

    #[test]
    fn flags_bec_reply_to_free_webmail_but_ignores_normal_mail() {
        let raw = "From: CEO <ceo@acme.co>\r\nReply-To: ceo.urgent@gmail.com\r\nTo: me@acme.co\r\nSubject: Wire\r\n\r\nNow";
        let m = MessageParser::default().parse(raw.as_bytes()).unwrap();
        let rt = vec![Addr { name: None, email: "ceo.urgent@gmail.com".into() }];
        let w = analyze_headers(&m, "CEO", "ceo@acme.co", &rt, None);
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].kind, "reply_to_mismatch");

        // Normal mail between same domain produces no warning.
        let clean = "From: Maya <maya@acme.co>\r\nTo: me@acme.co\r\nAuthentication-Results: mx; dmarc=pass spf=pass dkim=pass\r\n\r\nHi";
        let m2 = MessageParser::default().parse(clean.as_bytes()).unwrap();
        assert!(analyze_headers(&m2, "Maya", "maya@acme.co", &[], None).is_empty());
    }
}
