//! Account server auto-discovery: resolves IMAP and SMTP settings from an email address's domain.
//!
//! Lookup order (only the domain is ever sent, never the local-part):
//! 1. Domain's own HTTPS autoconfig (`https://autoconfig.<domain>/mail/config-v1.1.xml` and
//!    `https://<domain>/.well-known/autoconfig/mail/config-v1.1.xml`).
//! 2. Mozilla ISPDB (`https://autoconfig.thunderbird.net/v1.1/<domain>`).
//! 3. DNS MX lookup → known mail host mapping or Mozilla ISPDB lookup on the MX provider domain.
//! 4. DNS SRV lookup (RFC 6186: `_imaps._tcp` / `_submissions._tcp`).
//!
//! Settings from steps 1–2 come over HTTPS (`verified = true`). Settings inferred from DNS MX/SRV
//! carry `verified = false` so the UI asks the user to review and confirm them before connecting.

use crate::icons::is_public;
use crate::model::DiscoveredServers;
use hickory_resolver::TokioResolver;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::Url;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

const USER_AGENT: &str = "Mailvern/0.1 (+https://github.com/geraeldo/mailvern)";
const MAX_XML_BYTES: usize = 64 * 1024;

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

fn is_valid_domain(domain: &str) -> bool {
    !domain.is_empty()
        && domain.len() <= 253
        && domain.contains('.')
        && domain.parse::<IpAddr>().is_err()
        && domain.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        && domain.split('.').all(|l| !l.is_empty() && l.len() <= 63 && !l.starts_with('-') && !l.ends_with('-'))
}

fn extract_domain(email: &str) -> Option<String> {
    let (_, dom) = email.trim().rsplit_once('@')?;
    let dom = dom.trim().trim_end_matches('.').to_ascii_lowercase();
    is_valid_domain(&dom).then_some(dom)
}

fn org_domain(domain: &str) -> String {
    psl::domain_str(domain).unwrap_or(domain).to_string()
}

fn is_allowed_https_url(url: &Url) -> bool {
    if url.scheme() != "https" {
        return false;
    }
    let Some(host) = url.host_str() else { return false };
    is_valid_domain(host)
}

fn is_safe_server_host(host: &str) -> bool {
    let h = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if !is_valid_domain(&h) {
        return false;
    }
    !h.ends_with(".local") && !h.ends_with(".internal") && !h.ends_with(".localhost")
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ServerEntry {
    host: String,
    port: u16,
    socket: SocketType,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum SocketType {
    /// Implicit TLS (e.g. IMAP 993, SMTP 465). Preferred.
    Ssl = 0,
    /// STARTTLS (e.g. IMAP 143, SMTP 587).
    StartTls = 1,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ParsedConfig {
    provider_name: Option<String>,
    imap_host: String,
    imap_port: u16,
    smtp_host: String,
    smtp_port: u16,
}

/// Extracts the inner text of the first `<tag>…</tag>` block (case-insensitive tag name).
fn xml_tag<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let lower = xml.to_ascii_lowercase();
    let open_prefix = format!("<{}", tag.to_ascii_lowercase());
    let close = format!("</{}>", tag.to_ascii_lowercase());
    let mut search = 0;
    while let Some(i) = lower[search..].find(&open_prefix) {
        let start = search + i;
        let after = start + open_prefix.len();
        let next = lower.as_bytes().get(after).copied();
        if !matches!(next, Some(b'>' | b' ' | b'\t' | b'\r' | b'\n' | b'/')) {
            search = after;
            continue;
        }
        let gt = lower[after..].find('>')?;
        let body_start = after + gt + 1;
        if xml.as_bytes().get(body_start.saturating_sub(2)) == Some(&b'/') {
            search = body_start;
            continue;
        }
        let end_rel = lower[body_start..].find(&close)?;
        return Some(xml[body_start..body_start + end_rel].trim());
    }
    None
}

/// Yields `(opening_tag_attrs, inner_body)` for every `<tag ...>...</tag>` element.
fn xml_elements<'a>(xml: &'a str, tag: &str) -> Vec<(&'a str, &'a str)> {
    let lower = xml.to_ascii_lowercase();
    let open_prefix = format!("<{}", tag.to_ascii_lowercase());
    let close = format!("</{}>", tag.to_ascii_lowercase());
    let mut out = Vec::new();
    let mut search = 0;
    while let Some(i) = lower[search..].find(&open_prefix) {
        let start = search + i;
        let after = start + open_prefix.len();
        let next = lower.as_bytes().get(after).copied();
        if !matches!(next, Some(b'>' | b' ' | b'\t' | b'\r' | b'\n')) {
            search = after;
            continue;
        }
        let Some(gt) = lower[after..].find('>') else { break };
        let attrs = &xml[after..after + gt];
        let body_start = after + gt + 1;
        let Some(end_rel) = lower[body_start..].find(&close) else { break };
        let body = &xml[body_start..body_start + end_rel];
        out.push((attrs, body));
        search = body_start + end_rel + close.len();
    }
    out
}

fn attr_is(attrs: &str, name: &str, expected: &str) -> bool {
    let lc = attrs.to_ascii_lowercase();
    let needle = format!("{}=", name.to_ascii_lowercase());
    let Some(pos) = lc.find(&needle) else { return false };
    let rest = lc[pos + needle.len()..].trim_start();
    let val = if let Some(r) = rest.strip_prefix('"') {
        r.split('"').next().unwrap_or("")
    } else if let Some(r) = rest.strip_prefix('\'') {
        r.split('\'').next().unwrap_or("")
    } else {
        rest.split_whitespace().next().unwrap_or("")
    };
    val.eq_ignore_ascii_case(expected)
}

fn unescape_xml(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

fn parse_server_entry(body: &str, default_ssl_port: u16, default_starttls_port: u16) -> Option<ServerEntry> {
    let host = unescape_xml(xml_tag(body, "hostname")?).trim().trim_end_matches('.').to_ascii_lowercase();
    if !is_safe_server_host(&host) {
        return None;
    }
    let socket = match xml_tag(body, "socketType").unwrap_or("").trim().to_ascii_uppercase().as_str() {
        "SSL" => SocketType::Ssl,
        "STARTTLS" => SocketType::StartTls,
        _ => return None, // Refuse plaintext-only entries.
    };
    let port = xml_tag(body, "port")
        .and_then(|p| p.trim().parse::<u16>().ok())
        .filter(|&p| p > 0)
        .unwrap_or(match socket {
            SocketType::Ssl => default_ssl_port,
            SocketType::StartTls => default_starttls_port,
        });
    Some(ServerEntry { host, port, socket })
}

/// Parses Mozilla Autoconfig v1.1 XML (domain autoconfig or Thunderbird ISPDB).
fn parse_autoconfig_xml(xml: &str) -> Option<ParsedConfig> {
    if !xml.to_ascii_lowercase().contains("<clientconfig") {
        return None;
    }
    let provider_name = xml_tag(xml, "displayShortName")
        .or_else(|| xml_tag(xml, "displayName"))
        .map(unescape_xml)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s.len() <= 80);

    let mut imaps: Vec<ServerEntry> = xml_elements(xml, "incomingServer")
        .into_iter()
        .filter(|(attrs, _)| attr_is(attrs, "type", "imap"))
        .filter_map(|(_, body)| parse_server_entry(body, 993, 143))
        .collect();
    // Mailvern supports implicit TLS (993) and STARTTLS; prefer implicit TLS.
    imaps.sort_by_key(|s| (s.socket, s.port != 993));

    let mut smtps: Vec<ServerEntry> = xml_elements(xml, "outgoingServer")
        .into_iter()
        .filter(|(attrs, _)| attr_is(attrs, "type", "smtp"))
        .filter_map(|(_, body)| parse_server_entry(body, 465, 587))
        .collect();
    smtps.sort_by_key(|s| (s.socket, s.port != 465 && s.port != 587));

    let imap = imaps.into_iter().next()?;
    let smtp = smtps.into_iter().next()?;
    Some(ParsedConfig {
        provider_name,
        imap_host: imap.host,
        imap_port: imap.port,
        smtp_host: smtp.host,
        smtp_port: smtp.port,
    })
}

/// Known mail hosts identified from DNS MX records (for custom domains hosted on major providers).
pub(crate) fn known_mx_provider(mx_host: &str) -> Option<(&'static str, &'static str, u16, &'static str, u16)> {
    let mx = mx_host.trim().trim_end_matches('.').to_ascii_lowercase();
    let ends = |suffix: &str| mx == suffix || mx.ends_with(&format!(".{suffix}"));
    if ends("google.com") || ends("googlemail.com") {
        return Some(("Google Workspace", "imap.gmail.com", 993, "smtp.gmail.com", 465));
    }
    if ends("messagingengine.com") {
        return Some(("Fastmail", "imap.fastmail.com", 993, "smtp.fastmail.com", 465));
    }
    if ends("zoho.com") {
        return Some(("Zoho Mail", "imappro.zoho.com", 993, "smtppro.zoho.com", 465));
    }
    if ends("zoho.eu") {
        return Some(("Zoho Mail (EU)", "imappro.zoho.eu", 993, "smtppro.zoho.eu", 465));
    }
    if ends("zoho.in") {
        return Some(("Zoho Mail (IN)", "imappro.zoho.in", 993, "smtppro.zoho.in", 465));
    }
    if ends("zoho.com.au") {
        return Some(("Zoho Mail (AU)", "imappro.zoho.com.au", 993, "smtppro.zoho.com.au", 465));
    }
    if ends("icloud.com") {
        return Some(("iCloud Mail", "imap.mail.me.com", 993, "smtp.mail.me.com", 587));
    }
    if ends("migadu.com") {
        return Some(("Migadu", "imap.migadu.com", 993, "smtp.migadu.com", 465));
    }
    if ends("purelymail.com") {
        return Some(("Purelymail", "imap.purelymail.com", 993, "smtp.purelymail.com", 465));
    }
    if ends("mailbox.org") {
        return Some(("mailbox.org", "imap.mailbox.org", 993, "smtp.mailbox.org", 465));
    }
    if ends("posteo.de") {
        return Some(("Posteo", "posteo.de", 993, "posteo.de", 465));
    }
    if ends("protonmail.ch") {
        return Some(("Proton Bridge", "127.0.0.1", 1143, "127.0.0.1", 1025));
    }
    if ends("yahoodns.net") || ends("yahoo.com") {
        return Some(("Yahoo Mail", "imap.mail.yahoo.com", 993, "smtp.mail.yahoo.com", 465));
    }
    if ends("protection.outlook.com") || ends("outlook.com") {
        return Some(("Microsoft 365", "outlook.office365.com", 993, "smtp.office365.com", 587));
    }
    None
}

async fn fetch_xml(http: &reqwest::Client, url: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    if !is_allowed_https_url(&parsed) {
        return None;
    }
    let resp = http.get(parsed).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let bytes = resp.bytes().await.ok()?;
    if bytes.is_empty() || bytes.len() > MAX_XML_BYTES {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

async fn fetch_ispdb(http: &reqwest::Client, domain: &str) -> Option<ParsedConfig> {
    let url = format!("https://autoconfig.thunderbird.net/v1.1/{domain}");
    let xml = fetch_xml(http, &url).await?;
    parse_autoconfig_xml(&xml)
}

async fn lookup_srv(dns: &TokioResolver, name: &str) -> Option<(String, u16)> {
    let resp = dns.srv_lookup(name).await.ok()?;
    let mut records: Vec<_> = resp
        .answers()
        .iter()
        .filter_map(|r| match &r.data {
            hickory_resolver::proto::rr::RData::SRV(srv) => Some(srv),
            _ => None,
        })
        .collect();
    records.sort_by_key(|r| (r.priority, std::cmp::Reverse(r.weight)));
    for r in records {
        let host = r.target.to_ascii().trim_end_matches('.').to_ascii_lowercase();
        let port = r.port;
        if port > 0 && is_safe_server_host(&host) {
            return Some((host, port));
        }
    }
    None
}

/// Discovers IMAP/SMTP servers for `email` using domain autoconfig, Mozilla ISPDB, DNS MX, and DNS SRV.
pub async fn discover(email: &str) -> Option<DiscoveredServers> {
    let domain = extract_domain(email)?;
    let dns = Arc::new(TokioResolver::builder_tokio().ok()?.build().ok()?);
    let http = reqwest::Client::builder()
        .dns_resolver(PublicOnly(dns.clone()))
        .https_only(true)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 3 {
                attempt.error("too many redirects")
            } else if !is_allowed_https_url(attempt.url()) {
                attempt.error("disallowed redirect")
            } else {
                attempt.follow()
            }
        }))
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(5))
        .user_agent(USER_AGENT)
        .build()
        .ok()?;

    // 1. Domain's own HTTPS autoconfig endpoints (no email local-part leaked).
    let domain_urls = [
        format!("https://autoconfig.{domain}/mail/config-v1.1.xml"),
        format!("https://{domain}/.well-known/autoconfig/mail/config-v1.1.xml"),
    ];
    for url in &domain_urls {
        if let Some(xml) = fetch_xml(&http, url).await {
            if let Some(cfg) = parse_autoconfig_xml(&xml) {
                return Some(DiscoveredServers {
                    provider_name: cfg.provider_name,
                    imap_host: cfg.imap_host,
                    imap_port: cfg.imap_port,
                    smtp_host: cfg.smtp_host,
                    smtp_port: cfg.smtp_port,
                    source: "autoconfig_https".into(),
                    source_label: format!("Found via {domain} autoconfig (HTTPS)"),
                    verified: true,
                });
            }
        }
    }

    // 2. Mozilla Thunderbird ISPDB (try exact domain, then registrable org domain).
    let org = org_domain(&domain);
    let mut ispdb_candidates = vec![domain.clone()];
    if org != domain {
        ispdb_candidates.push(org.clone());
    }
    for cand in &ispdb_candidates {
        if let Some(cfg) = fetch_ispdb(&http, cand).await {
            return Some(DiscoveredServers {
                provider_name: cfg.provider_name,
                imap_host: cfg.imap_host,
                imap_port: cfg.imap_port,
                smtp_host: cfg.smtp_host,
                smtp_port: cfg.smtp_port,
                source: "ispdb".into(),
                source_label: "Found in Mozilla provider database".into(),
                verified: true,
            });
        }
    }

    // 3. DNS MX lookup → known mail host mapping or MX provider ISPDB.
    if let Ok(mx_resp) = dns.mx_lookup(domain.as_str()).await {
        let mut records: Vec<_> = mx_resp
            .answers()
            .iter()
            .filter_map(|r| match &r.data {
                hickory_resolver::proto::rr::RData::MX(mx) => Some(mx),
                _ => None,
            })
            .collect();
        records.sort_by_key(|r| r.preference);
        for rec in records.into_iter().take(3) {
            let mx_host = rec.exchange.to_ascii().trim_end_matches('.').to_ascii_lowercase();
            if !is_valid_domain(&mx_host) {
                continue;
            }
            if let Some((name, imap_host, imap_port, smtp_host, smtp_port)) = known_mx_provider(&mx_host) {
                return Some(DiscoveredServers {
                    provider_name: Some(name.into()),
                    imap_host: imap_host.into(),
                    imap_port,
                    smtp_host: smtp_host.into(),
                    smtp_port,
                    source: "mx_provider".into(),
                    source_label: format!("Detected {name} via DNS MX ({mx_host})"),
                    verified: false,
                });
            }
            let mx_org = org_domain(&mx_host);
            if mx_org != domain && mx_org != org {
                if let Some(cfg) = fetch_ispdb(&http, &mx_org).await {
                    return Some(DiscoveredServers {
                        provider_name: cfg.provider_name,
                        imap_host: cfg.imap_host,
                        imap_port: cfg.imap_port,
                        smtp_host: cfg.smtp_host,
                        smtp_port: cfg.smtp_port,
                        source: "mx_ispdb".into(),
                        source_label: format!("Detected via DNS MX ({mx_org})"),
                        verified: false,
                    });
                }
            }
        }
    }

    // 4. DNS SRV lookup (RFC 6186).
    let imap_name = format!("_imaps._tcp.{domain}");
    let smtp_name = format!("_submissions._tcp.{domain}");
    let (imap_srv, smtp_srv) = tokio::join!(lookup_srv(&dns, &imap_name), lookup_srv(&dns, &smtp_name));
    let smtp_srv = match smtp_srv {
        Some(s) => Some(s),
        None => {
            let fallback_name = format!("_submission._tcp.{domain}");
            lookup_srv(&dns, &fallback_name).await
        }
    };
    if let (Some((imap_host, imap_port)), Some((smtp_host, smtp_port))) = (imap_srv, smtp_srv) {
        return Some(DiscoveredServers {
            provider_name: None,
            imap_host,
            imap_port,
            smtp_host,
            smtp_port,
            source: "dns_srv".into(),
            source_label: format!("Found via DNS SRV records for {domain}"),
            verified: false,
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mozilla_autoconfig_xml_and_prefers_ssl() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <clientConfig version="1.1">
          <emailProvider id="fastmail.com">
            <displayName>Fastmail Mail</displayName>
            <displayShortName>Fastmail &amp; Co</displayShortName>
            <incomingServer type="imap">
              <hostname>imap.fastmail.com</hostname>
              <port>143</port>
              <socketType>STARTTLS</socketType>
            </incomingServer>
            <incomingServer type="imap">
              <hostname>imap.fastmail.com</hostname>
              <port>993</port>
              <socketType>SSL</socketType>
            </incomingServer>
            <outgoingServer type="smtp">
              <hostname>smtp.fastmail.com</hostname>
              <port>587</port>
              <socketType>STARTTLS</socketType>
            </outgoingServer>
            <outgoingServer type="smtp">
              <hostname>smtp.fastmail.com</hostname>
              <port>465</port>
              <socketType>SSL</socketType>
            </outgoingServer>
          </emailProvider>
        </clientConfig>"#;

        let cfg = parse_autoconfig_xml(xml).unwrap();
        assert_eq!(cfg.provider_name.as_deref(), Some("Fastmail & Co"));
        assert_eq!(cfg.imap_host, "imap.fastmail.com");
        assert_eq!(cfg.imap_port, 993);
        assert_eq!(cfg.smtp_host, "smtp.fastmail.com");
        assert_eq!(cfg.smtp_port, 465);
    }

    #[test]
    fn rejects_plaintext_and_local_autoconfig_hosts() {
        let plaintext = r#"<clientConfig version="1.1"><emailProvider>
            <incomingServer type="imap"><hostname>imap.example.com</hostname><port>143</port><socketType>plain</socketType></incomingServer>
            <outgoingServer type="smtp"><hostname>smtp.example.com</hostname><port>25</port><socketType>plain</socketType></outgoingServer>
        </emailProvider></clientConfig>"#;
        assert!(parse_autoconfig_xml(plaintext).is_none());

        let local = r#"<clientConfig version="1.1"><emailProvider>
            <incomingServer type="imap"><hostname>127.0.0.1</hostname><port>993</port><socketType>SSL</socketType></incomingServer>
            <outgoingServer type="smtp"><hostname>smtp.example.com</hostname><port>465</port><socketType>SSL</socketType></outgoingServer>
        </emailProvider></clientConfig>"#;
        assert!(parse_autoconfig_xml(local).is_none());
    }

    #[test]
    fn maps_known_mx_providers() {
        assert_eq!(
            known_mx_provider("aspmx.l.google.com."),
            Some(("Google Workspace", "imap.gmail.com", 993, "smtp.gmail.com", 465))
        );
        assert_eq!(
            known_mx_provider("in1-smtp.messagingengine.com"),
            Some(("Fastmail", "imap.fastmail.com", 993, "smtp.fastmail.com", 465))
        );
        assert_eq!(
            known_mx_provider("mx1.migadu.com"),
            Some(("Migadu", "imap.migadu.com", 993, "smtp.migadu.com", 465))
        );
        assert!(known_mx_provider("mail.unknown-custom-host.example").is_none());
    }

    #[test]
    fn validates_email_domains() {
        assert_eq!(extract_domain("Alex@Acme.Co"), Some("acme.co".into()));
        assert_eq!(extract_domain("user@localhost"), None);
        assert_eq!(extract_domain("user@127.0.0.1"), None);
        assert_eq!(extract_domain("not-an-email"), None);
    }
}
