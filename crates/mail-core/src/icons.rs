//! Sender pictures: a domain's BIMI logo, or else its website icon.
//!
//! Everything is fetched straight from the sender's own domain, never through a
//! third-party icon service, so no one else learns who writes to you. Addresses at
//! personal mail providers (Gmail, Outlook…) get no picture: their logo says nothing
//! about the person.

use hickory_resolver::TokioResolver;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::{Response, Url};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

/// A picture for a sender's domain.
#[derive(Clone, Debug)]
pub struct Icon {
    pub mime: String,
    pub data: Vec<u8>,
    /// A BIMI logo: made to fill a round avatar. Website icons are drawn smaller, on a plate.
    pub logo: bool,
}

pub enum Outcome {
    Found(Icon),
    /// The domain answered and has no usable picture.
    Missing,
    /// Network trouble (offline, timeouts): worth asking again soon.
    Failed,
}

const MAX_IMAGE: usize = 256 * 1024;
const MAX_HTML: usize = 384 * 1024;
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Mailvern/0.1";

/// Personal mail providers. Matched on the registrable domain, or its first label for
/// providers with a domain per country (yahoo.co.id, gmx.de…).
const PERSONAL: &[&str] = &[
    "gmail.com", "googlemail.com", "outlook.com", "hotmail.com", "live.com", "msn.com", "passport.com",
    "icloud.com", "me.com", "mac.com", "aol.com", "aim.com", "ymail.com", "rocketmail.com",
    "proton.me", "protonmail.com", "protonmail.ch", "pm.me", "tutanota.com", "tuta.io", "tutamail.com",
    "mail.com", "gmx.com", "gmx.net", "zoho.com", "zohomail.com", "fastmail.com", "fastmail.fm", "hey.com",
    "yandex.com", "yandex.ru", "mail.ru", "inbox.ru", "list.ru", "bk.ru", "qq.com", "163.com", "126.com",
    "yeah.net", "sina.com", "naver.com", "daum.net", "hanmail.net", "web.de", "t-online.de", "orange.fr",
    "free.fr", "laposte.net", "libero.it", "virgilio.it", "btinternet.com", "comcast.net", "att.net",
    "verizon.net", "sbcglobal.net", "cox.net", "mailinator.com", "duck.com", "skiff.com", "hushmail.com",
];
const PERSONAL_LABELS: &[&str] = &["yahoo", "hotmail", "outlook", "live", "gmx", "yandex", "rediffmail"];

/// The domain to look up for an address, or None when it gets no picture.
pub fn icon_domain(addr: &str) -> Option<String> {
    let domain = addr.rsplit('@').next()?.trim().trim_end_matches('.').to_ascii_lowercase();
    if domain.len() > 253
        || !domain.contains('.')
        || domain.parse::<IpAddr>().is_ok()
        || !domain.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        || domain.split('.').any(|l| l.is_empty() || l.len() > 63)
    {
        return None;
    }
    let org = org_domain(&domain);
    let first = org.split('.').next().unwrap_or("");
    if PERSONAL.contains(&org) || PERSONAL_LABELS.contains(&first) {
        return None;
    }
    Some(domain)
}

/// Only HTTPS URLs targeting a domain name (never an IP literal, which bypasses DNS resolvers).
fn is_allowed_url(url: &Url) -> bool {
    if url.scheme() != "https" {
        return false;
    }
    let Some(host) = url.host_str() else { return false };
    !host.is_empty() && !host.starts_with('[') && host.parse::<IpAddr>().is_err()
}

/// The registrable domain: `news.stripe.com` → `stripe.com`, `mail.example.co.id` → `example.co.id`.
fn org_domain(domain: &str) -> &str {
    psl::domain_str(domain).unwrap_or(domain)
}

pub struct Fetcher {
    dns: Arc<TokioResolver>,
    http: reqwest::Client,
    /// Keeps a burst of new senders from opening dozens of connections at once.
    limit: Semaphore,
}

impl Fetcher {
    pub fn new() -> anyhow::Result<Self> {
        let dns = Arc::new(TokioResolver::builder_tokio()?.build()?);
        let http = reqwest::Client::builder()
            .dns_resolver(PublicOnly(dns.clone()))
            .https_only(true)
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= 5 {
                    attempt.error("too many redirects")
                } else if !is_allowed_url(attempt.url()) {
                    attempt.error("disallowed redirect target")
                } else {
                    attempt.follow()
                }
            }))
            .connect_timeout(Duration::from_secs(4))
            .timeout(Duration::from_secs(8))
            .user_agent(USER_AGENT)
            .build()?;
        Ok(Fetcher { dns, http, limit: Semaphore::new(6) })
    }

    pub async fn fetch(&self, domain: &str) -> Outcome {
        let _permit = self.limit.acquire().await;
        let mut failed = false;
        let org = org_domain(domain);

        if let Some(icon) = self.bimi(domain, org, &mut failed).await {
            return Outcome::Found(icon);
        }
        // Mail often comes from a subdomain without a website (em.example.com): the
        // organization's own site comes first.
        let mut hosts = vec![org];
        if domain != org {
            hosts.push(domain);
        }
        for host in hosts {
            if let Some(icon) = self.site_icon(host, &mut failed).await {
                return Outcome::Found(icon);
            }
        }
        if failed {
            Outcome::Failed
        } else {
            Outcome::Missing
        }
    }

    // ---------- BIMI ----------

    async fn bimi(&self, domain: &str, org: &str, failed: &mut bool) -> Option<Icon> {
        let mut names = vec![domain];
        if org != domain {
            names.push(org);
        }
        for name in names {
            match self.bimi_record(name).await {
                Ok(Some(record)) => {
                    // A record decides for its domain, even one that declines a logo (`l=`).
                    let url = bimi_location(&record)?;
                    return self.image(url, failed).await.map(|mut i| {
                        i.logo = true;
                        i
                    });
                }
                Ok(None) => continue,
                Err(()) => {
                    *failed = true;
                    return None;
                }
            }
        }
        None
    }

    /// The domain's `v=BIMI1` record, None when it has none, Err on DNS trouble.
    async fn bimi_record(&self, name: &str) -> Result<Option<String>, ()> {
        let lookup = match self.dns.txt_lookup(format!("default._bimi.{name}.")).await {
            Ok(l) => l,
            Err(e) if e.is_no_records_found() => return Ok(None),
            Err(_) => return Err(()),
        };
        for record in lookup.answers() {
            if let hickory_resolver::proto::rr::RData::TXT(txt) = &record.data {
                let text: String = txt.txt_data.iter().map(|s| String::from_utf8_lossy(s)).collect();
                if text.trim_start().to_ascii_lowercase().starts_with("v=bimi1") {
                    return Ok(Some(text));
                }
            }
        }
        Ok(None)
    }

    // ---------- website icons ----------

    async fn site_icon(&self, host: &str, failed: &mut bool) -> Option<Icon> {
        let home = Url::parse(&format!("https://{host}/")).ok()?;
        let base = match self.get(home).await {
            Ok(resp) => {
                let base = resp.url().clone();
                let html = resp.status().is_success()
                    && resp
                        .headers()
                        .get(reqwest::header::CONTENT_TYPE)
                        .and_then(|v| v.to_str().ok())
                        .is_some_and(|t| t.contains("html"));
                if html {
                    if let Ok(Some(body)) = read(resp, MAX_HTML, true).await {
                        for url in icon_links(&String::from_utf8_lossy(&body), &base) {
                            if let Some(icon) = self.image(url, failed).await {
                                return Some(icon);
                            }
                        }
                    }
                }
                base
            }
            Err(e) => {
                if e.is_timeout() || e.is_connect() {
                    *failed = true;
                }
                // No site to speak of: /favicon.ico won't be there either.
                return None;
            }
        };
        self.image(base.join("/favicon.ico").ok()?, failed).await
    }

    async fn get(&self, url: Url) -> reqwest::Result<Response> {
        self.http.get(url).header(reqwest::header::ACCEPT, "text/html,image/*;q=0.9,*/*;q=0.5").send().await
    }

    /// Downloads an image, checking by its bytes that it is one.
    async fn image(&self, url: Url, failed: &mut bool) -> Option<Icon> {
        if !is_allowed_url(&url) {
            return None;
        }
        let resp = match self.get(url).await {
            Ok(r) if r.status().is_success() => r,
            Ok(_) => return None,
            Err(e) => {
                if e.is_timeout() || e.is_connect() {
                    *failed = true;
                }
                return None;
            }
        };
        let data = read(resp, MAX_IMAGE, false).await.ok()??;
        let mime = sniff(&data)?;
        Some(Icon { mime: mime.to_string(), data, logo: false })
    }
}

/// The `l=` URL of a BIMI record, when it names an allowed https domain URL.
fn bimi_location(record: &str) -> Option<Url> {
    record
        .split(';')
        .filter_map(|tag| tag.split_once('='))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case("l"))
        .map(|(_, v)| v.trim())
        .filter(|v| !v.is_empty())
        .and_then(|v| Url::parse(v).ok())
        .filter(is_allowed_url)
}

/// Reads a body up to `max` bytes. Past that it is cut (`truncate`) or dropped (None).
async fn read(mut resp: Response, max: usize, truncate: bool) -> reqwest::Result<Option<Vec<u8>>> {
    if !truncate && resp.content_length().is_some_and(|n| n as usize > max) {
        return Ok(None);
    }
    let mut buf = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        buf.extend_from_slice(&chunk);
        if buf.len() > max {
            if truncate {
                buf.truncate(max);
                break;
            }
            return Ok(None);
        }
    }
    Ok(Some(buf))
}

/// The image type, told from the bytes rather than the server's word.
fn sniff(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if data.len() > 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Some("image/webp")
    } else if data.len() > 6 && data[..4] == [0, 0, 1, 0] && data[4..6] != [0, 0] {
        Some("image/x-icon")
    } else {
        let head = String::from_utf8_lossy(&data[..data.len().min(2048)]).to_ascii_lowercase();
        let mut s = head.trim_start_matches('\u{feff}').trim_start();
        loop {
            if let Some(rest) = s.strip_prefix("<?xml") {
                let end = rest.find("?>")?;
                s = rest[end + 2..].trim_start();
            } else if let Some(rest) = s.strip_prefix("<!--") {
                let end = rest.find("-->")?;
                s = rest[end + 3..].trim_start();
            } else if let Some(rest) = s.strip_prefix("<!doctype svg") {
                let end = rest.find('>')?;
                s = rest[end + 1..].trim_start();
            } else {
                break;
            }
        }
        let is_svg = s.strip_prefix("<svg").is_some_and(|r| r.is_empty() || r.starts_with(|c: char| c.is_ascii_whitespace() || c == '>' || c == '/'));
        is_svg.then_some("image/svg+xml")
    }
}

/// Icon links in a page's head, best first: large or vector icons before 16px favicons.
fn icon_links(html: &str, base: &Url) -> Vec<Url> {
    let lower = html.to_ascii_lowercase();
    let end = lower.find("</head").unwrap_or(lower.len());
    let mut found: Vec<(u32, Url)> = Vec::new();
    let mut at = 0;
    while let Some(i) = lower[at..end].find("<link") {
        let start = at + i;
        let Some(close) = lower[start..end].find('>') else { break };
        let tag = &html[start..start + close];
        at = start + close;

        let attr = |name: &str| attribute(tag, name);
        let rel = attr("rel").unwrap_or_default().to_ascii_lowercase();
        let rels: Vec<&str> = rel.split_ascii_whitespace().collect();
        let touch = rels.iter().any(|r| r.starts_with("apple-touch-icon"));
        if !touch && !rels.contains(&"icon") {
            continue; // also skips mask-icon: a one-colour silhouette
        }
        let Some(href) = attr("href") else { continue };
        let Ok(url) = base.join(&href.replace("&amp;", "&")) else { continue };
        if !is_allowed_url(&url) {
            continue;
        }
        let svg = attr("type").is_some_and(|t| t.contains("svg")) || url.path().ends_with(".svg");
        let size = attr("sizes")
            .and_then(|s| s.split_ascii_whitespace().filter_map(|d| d.split(['x', 'X']).next()?.parse::<u32>().ok()).max());
        let score = if svg {
            300
        } else {
            match size {
                Some(n) => n.min(512),
                None if touch => 180,
                None => 32,
            }
        };
        found.push((score, url));
    }
    found.sort_by_key(|a| std::cmp::Reverse(a.0));
    found.into_iter().map(|(_, u)| u).take(4).collect()
}

/// An attribute's value in a tag, quoted or not.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(name) {
        let pos = from + i;
        from = pos + name.len();
        let before = lower[..pos].chars().last();
        if !before.is_some_and(|c| c.is_ascii_whitespace()) {
            continue;
        }
        let rest = tag[from..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else { continue };
        let rest = rest.trim_start();
        let value = match rest.chars().next()? {
            q @ ('"' | '\'') => rest[1..].split(q).next()?,
            _ => rest.split(|c: char| c.is_ascii_whitespace()).next()?.trim_end_matches('/'),
        };
        return Some(value.trim().to_string());
    }
    None
}

/// Resolves names for the HTTP client, refusing addresses on this machine or its local
/// network: a sender's domain must not get Mailvern to call into your router or a dev server.
struct PublicOnly(Arc<TokioResolver>);

impl Resolve for PublicOnly {
    fn resolve(&self, name: Name) -> Resolving {
        let dns = self.0.clone();
        let host = name.as_str().to_string();
        Box::pin(async move {
            let ips = dns.lookup_ip(host.as_str()).await?;
            let addrs: Vec<SocketAddr> = ips.iter().filter(is_public).map(|ip| SocketAddr::new(ip, 0)).collect();
            if addrs.is_empty() {
                return Err("no public address".into());
            }
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

fn is_public(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_unspecified()
                || v4.is_multicast()
                || o[0] == 0
                || (o[0] == 100 && (o[1] & 0xC0) == 64)
                || (o[0] == 192 && o[1] == 0 && o[2] == 0)
                || (o[0] == 198 && (o[1] == 18 || o[1] == 19))
                || o[0] >= 240)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public(&IpAddr::V4(v4));
            }
            let s = v6.segments();
            // Reject loopback, unspecified, multicast, ULA, link-local, site-local,
            // IPv4-compatible (::/96), NAT64 (64:ff9b::/96, 64:ff9b:1::/48), Teredo (2001::/32),
            // documentation (2001:db8::/32), and 6to4 (2002::/16).
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (s[0] & 0xFE00) == 0xFC00
                || (s[0] & 0xFFC0) == 0xFE80
                || (s[0] & 0xFFC0) == 0xFEC0
                || (s[0] == 0 && s[1] == 0 && s[2] == 0 && s[3] == 0 && s[4] == 0 && s[5] == 0)
                || (s[0] == 0x0064 && s[1] == 0xFF9B && ((s[2] == 0 && s[3] == 0 && s[4] == 0 && s[5] == 0) || s[2] == 1))
                || (s[0] == 0x2001 && (s[1] == 0 || s[1] == 0x0DB8))
                || s[0] == 0x2002)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn personal_and_bad_domains_get_no_picture() {
        assert_eq!(icon_domain("a@gmail.com"), None);
        assert_eq!(icon_domain("a@yahoo.co.id"), None);
        assert_eq!(icon_domain("a@gmx.de"), None);
        assert_eq!(icon_domain("a@localhost"), None);
        assert_eq!(icon_domain("a@127.0.0.1"), None);
        assert_eq!(icon_domain("a@192.168.1.1"), None);
        assert_eq!(icon_domain("a@exa mple.com"), None);
        assert_eq!(icon_domain("Bills@News.Stripe.com").as_deref(), Some("news.stripe.com"));
        assert_eq!(org_domain("mail.example.co.id"), "example.co.id");
    }

    #[test]
    fn bimi_record_location() {
        let u = bimi_location("v=BIMI1; l=https://x.com/logo.svg; a=").unwrap();
        assert_eq!(u.as_str(), "https://x.com/logo.svg");
        assert!(bimi_location("v=BIMI1; l=;").is_none());
        assert!(bimi_location("v=BIMI1; l=http://x.com/logo.svg").is_none());
        assert!(bimi_location("v=BIMI1; l=https://127.0.0.1/logo.svg").is_none());
        assert!(bimi_location("v=BIMI1; l=https://[::1]/logo.svg").is_none());
    }

    #[test]
    fn picks_the_best_icon_link() {
        let base = Url::parse("https://example.com/en/").unwrap();
        let html = r#"<html><head>
            <link rel="icon" href="/favicon-16.png" sizes="16x16">
            <link rel=mask-icon href="/mask.svg">
            <LINK REL="apple-touch-icon" HREF="touch.png">
            <link rel="shortcut icon" href="http://example.com/old.ico">
            <link href='/icon.svg' rel='icon' type='image/svg+xml'>
            </head><body><link rel="icon" href="/late.png"></body>"#;
        let urls: Vec<String> = icon_links(html, &base).into_iter().map(|u| u.to_string()).collect();
        assert_eq!(urls, ["https://example.com/icon.svg", "https://example.com/en/touch.png", "https://example.com/favicon-16.png"]);
    }

    #[test]
    fn sniffs_images() {
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\nrest"), Some("image/png"));
        assert_eq!(sniff(b"<?xml version=\"1.0\"?><svg xmlns=\"\"/>"), Some("image/svg+xml"));
        assert_eq!(sniff(&[0, 0, 1, 0, 1, 0, 16]), Some("image/x-icon"));
        assert_eq!(sniff(b"<!doctype html><html>"), None);
        assert_eq!(sniff(b"<html><body><svg></svg></body></html>"), None);
    }

    #[test]
    fn local_addresses_are_refused() {
        for ip in [
            "127.0.0.1", "10.1.2.3", "192.168.1.1", "169.254.1.1", "100.64.0.1", "198.18.0.1",
            "::1", "fd00::1", "::ffff:192.168.0.1", "64:ff9b::127.0.0.1", "2002:7f00:1::1", "2001:db8::1",
        ] {
            assert!(!is_public(&ip.parse().unwrap()), "{ip}");
        }
        assert!(is_public(&"93.184.216.34".parse().unwrap()));
        assert!(is_public(&"2606:4700::1111".parse().unwrap()));
    }
}
