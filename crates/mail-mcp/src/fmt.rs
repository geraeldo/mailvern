//! How mail is written out for an agent: compact, readable text rather than JSON.
//! Mirrors what Ask AI shows its model (`src/lib/ai/search.svelte.ts`).

use anyhow::{anyhow, Result};
use chrono::{Local, NaiveDate, NaiveDateTime, TimeZone};
use mail_core::model::{Account, Addr, AgentMode, AgentPermissions, Folder, MessageSummary, SnoozeUntil};
use std::collections::HashMap;

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

/// Parses a snooze target: "laterToday", "tomorrow", "nextWeek", or a local date/time.
pub fn parse_snooze(s: &str) -> Result<i64> {
    let trimmed = s.trim();
    let now = Local::now();
    match trimmed {
        "laterToday" => Ok(mail_core::rules::snooze_until(SnoozeUntil::LaterToday, now)),
        "tomorrow" => Ok(mail_core::rules::snooze_until(SnoozeUntil::Tomorrow, now)),
        "nextWeek" => Ok(mail_core::rules::snooze_until(SnoozeUntil::NextWeek, now)),
        _ => to_unix(trimmed, "until"),
    }
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

/// The user's accounts and folders, for naming things by id, and how their sync is doing.
pub struct Names {
    pub accounts: Vec<Account>,
    pub folders: Vec<Folder>,
    /// Account → latest sync state and error message, where known.
    pub sync: HashMap<i64, (String, Option<String>)>,
    /// Effective MCP permissions (global default + per-account overrides).
    pub perms: AgentPermissions,
}

impl Names {
    pub fn load(ctx: &crate::Ctx) -> Result<Names> {
        Ok(Names {
            accounts: ctx.engine.accounts()?,
            folders: ctx.engine.folders()?,
            sync: ctx.sync.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            perms: ctx.engine.agent_permissions()?,
        })
    }

    /// Why an account's local mail can't be relied on, if so: it can't sync, or nothing has
    /// been downloaded. A sync in progress is not trouble; mail is checked all the time.
    fn trouble(&self, a: &Account) -> Option<String> {
        match self.sync.get(&a.id) {
            Some((state, message)) if state == "error" => Some(format!("can't sync: {}", message.as_deref().unwrap_or("unknown error"))),
            _ if !self.folders.iter().any(|f| f.account_id == a.id) => Some("nothing downloaded yet".into()),
            _ => None,
        }
    }

    fn syncing(&self, a: &Account) -> bool {
        self.sync.get(&a.id).is_some_and(|(state, _)| state == "syncing")
    }

    /// A line for search results when some account's mail may be missing, so "nothing found"
    /// isn't taken for "no such mail".
    pub fn warning(&self) -> Option<String> {
        let bad: Vec<String> = self
            .accounts
            .iter()
            .filter(|a| self.perms.is_account_allowed(a.id))
            .filter_map(|a| self.trouble(a).map(|t| format!("{} ({t})", a.email)))
            .collect();
        (!bad.is_empty()).then(|| format!("Note: Mailvern's copy of the mail may be incomplete for {}. Results can be missing until it has synced; tell the user rather than concluding the mail doesn't exist.", bad.join(", ")))
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
        let allowed_accounts: Vec<&Account> = self.accounts.iter().filter(|a| self.perms.is_account_allowed(a.id)).collect();
        if allowed_accounts.is_empty() {
            return "No mail accounts are shared with agents. Enable accounts in Mailvern › Settings › Agent access.".into();
        }
        allowed_accounts
            .into_iter()
            .map(|a| {
                let label = [a.label.as_str(), a.name.as_str()].into_iter().find(|s| !s.is_empty()).unwrap_or("");
                let mut hidden_spam_trash: Vec<&str> = Vec::new();
                let mut folders: Vec<String> = Vec::new();
                for f in self.folders.iter().filter(|f| f.account_id == a.id) {
                    if !self.perms.is_folder_allowed(f) {
                        match f.role.as_deref() {
                            Some("junk") if !hidden_spam_trash.contains(&"Spam") => hidden_spam_trash.push("Spam"),
                            Some("trash") if !hidden_spam_trash.contains(&"Trash") => hidden_spam_trash.push("Trash"),
                            _ => {}
                        }
                        continue;
                    }
                    let role = f.role.as_deref().map(|r| format!(" [{r}]")).unwrap_or_default();
                    let unread = if f.unread > 0 { format!(" · {} unread", f.unread) } else { String::new() };
                    folders.push(format!("  folder {}: {}{role}{unread}", f.id, f.name));
                }
                if !hidden_spam_trash.is_empty() {
                    folders.push(format!(
                        "  ({} hidden by default — call request_folder_access if the user asks to check)",
                        hidden_spam_trash.join(" and ")
                    ));
                }
                let label = if label.is_empty() { String::new() } else { format!("{label} ") };
                let mode = match self.perms.for_account(a.id) {
                    AgentMode::Approval => "ask before acting",
                    AgentMode::Auto => "automatic",
                };
                let state = self.trouble(a).or_else(|| self.syncing(a).then(|| "checking for mail now".to_string()));
                let trouble = state.map(|t| format!(" · {t}")).unwrap_or_default();
                let folders = if folders.is_empty() { "  (no folders yet)".to_string() } else { folders.join("\n") };
                format!("Account {}: {label}<{}> · {mode}{trouble}\n{folders}", a.id, a.email)
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
    fn listing_says_when_mail_is_missing() {
        let acc = |id: i64, email: &str| Account {
            id,
            name: String::new(),
            email: email.into(),
            imap_host: String::new(),
            imap_port: 993,
            smtp_host: String::new(),
            smtp_port: 465,
            color: String::new(),
            signature: String::new(),
            position: id,
            label: String::new(),
            ..Default::default()
        };
        let folder = |id: i64, account_id: i64| Folder { id, account_id, path: "INBOX".into(), name: "Inbox".into(), role: Some("inbox".into()), unread: 2, position: None };
        let mut names = Names {
            accounts: vec![acc(1, "a@x.co"), acc(2, "b@x.co"), acc(3, "c@x.co")],
            folders: vec![folder(5, 1), folder(6, 3)],
            sync: HashMap::new(),
            perms: AgentPermissions::default(),
        };
        names.perms.account_modes.insert(2, AgentMode::Auto);
        // A routine check for mail is shown in the list but is no reason to doubt results.
        names.sync.insert(1, ("syncing".into(), None));
        names.sync.insert(3, ("error".into(), Some("Sign-in failed".into())));
        assert_eq!(
            names.listing(),
            "Account 1: <a@x.co> · ask before acting · checking for mail now\n  folder 5: Inbox [inbox] · 2 unread\n\nAccount 2: <b@x.co> · automatic · nothing downloaded yet\n  (no folders yet)\n\nAccount 3: <c@x.co> · ask before acting · can't sync: Sign-in failed\n  folder 6: Inbox [inbox] · 2 unread"
        );
        let warning = names.warning().unwrap();
        assert!(warning.contains("b@x.co (nothing downloaded yet), c@x.co (can't sync: Sign-in failed)"), "{warning}");
        names.accounts.truncate(1);
        assert!(names.warning().is_none());
    }

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
