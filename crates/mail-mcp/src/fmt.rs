//! How mail is written out for an agent: compact, readable text rather than JSON.
//! Mirrors what Ask AI shows its model (`src/lib/ai/search.svelte.ts`).

use anyhow::{anyhow, Result};
use chrono::{Local, NaiveDate, NaiveDateTime, TimeZone};
use mail_core::model::{Account, Addr, Folder, MessageSummary};
use mail_core::Engine;

/// "2026-11-05 14:30 Thu", in local time.
pub fn stamp(unix: i64) -> String {
    Local.timestamp_opt(unix, 0).single().map_or_else(|| "unknown date".into(), |d| d.format("%Y-%m-%d %H:%M %a").to_string())
}

/// "2026-11-05" or "2026-11-05T14:30" in local time, to unix seconds.
pub fn to_unix(s: &str, label: &str) -> Result<i64> {
    let s = s.trim();
    let bad = || anyhow!("{label} must look like 2026-11-05 or 2026-11-05T14:30");
    let naive = if s.len() <= 10 {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| bad())?.and_hms_opt(0, 0, 0).ok_or_else(bad)?
    } else {
        let head: String = s.chars().take(16).collect::<String>().replace(' ', "T");
        NaiveDateTime::parse_from_str(&head, "%Y-%m-%dT%H:%M").map_err(|_| bad())?
    };
    Local.from_local_datetime(&naive).earliest().map(|d| d.timestamp()).ok_or_else(bad)
}

pub fn who(name: Option<&str>, addr: &str) -> String {
    match name.filter(|n| !n.trim().is_empty()) {
        Some(n) => format!("{n} <{addr}>"),
        None => addr.to_string(),
    }
}

pub fn addr_list(xs: &[Addr]) -> String {
    xs.iter().map(|a| who(a.name.as_deref(), &a.email)).collect::<Vec<_>>().join(", ")
}

/// At most `max` characters from `offset`, with a note saying how to read on.
pub fn clip(s: &str, max: usize, offset: usize) -> String {
    let total = s.chars().count();
    let offset = offset.min(total);
    let part: String = s.chars().skip(offset).take(max).collect();
    let end = offset + part.chars().count();
    if end < total {
        format!("{part}\n\n[… {} more characters. Read on with offset {end}.]", total - end)
    } else {
        part
    }
}

pub fn kb(bytes: usize) -> String {
    if bytes < 1024 * 1024 {
        format!("{} KB", (bytes / 1024).max(1))
    } else {
        format!("{:.1} MB", bytes as f64 / 1024.0 / 1024.0)
    }
}

/// The user's accounts and folders, for naming things by id.
pub struct Names {
    pub accounts: Vec<Account>,
    pub folders: Vec<Folder>,
}

impl Names {
    pub fn load(engine: &Engine) -> Result<Names> {
        Ok(Names { accounts: engine.accounts()?, folders: engine.folders()? })
    }

    pub fn account(&self, id: i64) -> Option<&Account> {
        self.accounts.iter().find(|a| a.id == id)
    }

    /// "Inbox", "Spam [junk]", and the account when there are several.
    pub fn folder(&self, id: i64) -> String {
        let Some(f) = self.folders.iter().find(|f| f.id == id) else { return format!("folder {id}") };
        let mut s = f.name.clone();
        if let Some(role) = f.role.as_deref().filter(|r| *r != f.name.to_lowercase()) {
            s.push_str(&format!(" [{role}]"));
        }
        if self.accounts.len() > 1 {
            if let Some(a) = self.account(f.account_id) {
                s.push_str(&format!(" · {}", a.email));
            }
        }
        s
    }

    pub fn listing(&self) -> String {
        if self.accounts.is_empty() {
            return "No mail accounts are set up in Mailvern yet.".into();
        }
        self.accounts
            .iter()
            .map(|a| {
                let label = [a.label.as_str(), a.name.as_str()].into_iter().find(|s| !s.is_empty()).unwrap_or("");
                let folders: Vec<String> = self
                    .folders
                    .iter()
                    .filter(|f| f.account_id == a.id)
                    .map(|f| {
                        let role = f.role.as_deref().map(|r| format!(" [{r}]")).unwrap_or_default();
                        let unread = if f.unread > 0 { format!(" · {} unread", f.unread) } else { String::new() };
                        format!("  folder {}: {}{role}{unread}", f.id, f.name)
                    })
                    .collect();
                let label = if label.is_empty() { String::new() } else { format!("{label} ") };
                format!("Account {}: {label}<{}>\n{}", a.id, a.email, folders.join("\n"))
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

/// One search result.
pub fn row(m: &MessageSummary, names: &Names) -> String {
    let flags: Vec<&str> = [(!m.seen, "unread"), (m.flagged, "flagged"), (m.has_attachments, "has attachments"), (m.snoozed_until.is_some(), "snoozed")]
        .into_iter()
        .filter_map(|(on, name)| on.then_some(name))
        .collect();
    let mut s = format!("#{} · {} · {}", m.id, stamp(m.date), names.folder(m.folder_id));
    if !flags.is_empty() {
        s.push_str(&format!(" · {}", flags.join(", ")));
    }
    s.push_str(&format!("\n  From: {}", who(Some(&m.from_name), &m.from_addr)));
    s.push_str(&format!("\n  Subject: {}", subject(&m.subject)));
    if !m.snippet.is_empty() {
        s.push_str(&format!("\n  {}", m.snippet.chars().take(180).collect::<String>()));
    }
    s
}

pub fn subject(s: &str) -> &str {
    if s.trim().is_empty() { "(no subject)" } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_clipping() {
        let day = to_unix("2026-11-05", "after").unwrap();
        assert_eq!(stamp(day), "2026-11-05 00:00 Thu");
        assert_eq!(to_unix("2026-11-05T14:30", "after").unwrap() - day, 14 * 3600 + 30 * 60);
        assert_eq!(to_unix("2026-11-05 14:30:59", "after").unwrap() - day, 14 * 3600 + 30 * 60);
        assert!(to_unix("last week", "after").unwrap_err().to_string().starts_with("after must look like"));
        assert_eq!(clip("héllo wörld", 5, 0), "héllo\n\n[… 6 more characters. Read on with offset 5.]");
        assert_eq!(clip("héllo wörld", 50, 6), "wörld");
        assert_eq!(clip("abc", 5, 99), "");
        assert_eq!(who(Some(""), "a@b.co"), "a@b.co");
        assert_eq!(who(Some("Budi"), "a@b.co"), "Budi <a@b.co>");
    }
}
