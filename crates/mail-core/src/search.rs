//! Human-facing search query parser: turns Gmail/Mailspring-style operators
//! (`from:`, `to:`, `subject:`, `has:attachment`, `is:unread`, `is:flagged`,
//! `in:sent`, `after:`, `before:`, `older_than:`, `newer_than:`) into a `MailQuery`.

use crate::model::{MailQuery, SearchScope, UNIFIED_ROLES};
use chrono::{Datelike, Duration, Local, NaiveDate, TimeZone};

/// Parsed human search query, plus whether any structured operator was recognized.
#[derive(Clone, Debug)]
pub struct ParsedSearch {
    pub query: MailQuery,
    pub has_operators: bool,
}

/// Tokenizes a search string into words and `key:"quoted value"` pairs.
fn tokenize(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                cur.push(c);
            }
            c if c.is_whitespace() && !in_quotes => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn unquote(s: &str) -> String {
    let t = s.trim();
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        t[1..t.len() - 1].trim().to_string()
    } else {
        t.trim_matches('"').trim().to_string()
    }
}

/// Parses relative durations like `7d`, `2w`, `3m`, `1y` into a unix timestamp before `now`.
fn parse_relative(val: &str, now: chrono::DateTime<Local>) -> Option<i64> {
    let v = val.trim().to_ascii_lowercase();
    if v.len() < 2 {
        return None;
    }
    let unit = v.chars().last()?;
    let n: i64 = v[..v.len() - 1].parse().ok()?;
    if n <= 0 {
        return None;
    }
    let days = match unit {
        'd' => n,
        'w' => n.saturating_mul(7),
        'm' => n.saturating_mul(30),
        'y' => n.saturating_mul(365),
        _ => return None,
    };
    Some((now - Duration::days(days)).timestamp())
}

/// Parses a date token (`YYYY-MM-DD`, `YYYY/MM/DD`, `today`, `yesterday`, or `7d`/`2w`/`3m`/`1y`).
/// When `exclusive_end` is true, date-only values resolve to the start of the day (for `before:`).
fn parse_date(val: &str, now: chrono::DateTime<Local>) -> Option<i64> {
    let v = val.trim().to_ascii_lowercase();
    if v == "today" {
        let d = now.date_naive().and_hms_opt(0, 0, 0)?;
        return now.timezone().from_local_datetime(&d).earliest().map(|t| t.timestamp());
    }
    if v == "yesterday" {
        let d = (now.date_naive() - Duration::days(1)).and_hms_opt(0, 0, 0)?;
        return now.timezone().from_local_datetime(&d).earliest().map(|t| t.timestamp());
    }
    if let Some(rel) = parse_relative(&v, now) {
        return Some(rel);
    }
    let norm = v.replace('/', "-");
    if let Ok(nd) = NaiveDate::parse_from_str(&norm, "%Y-%m-%d") {
        let dt = nd.and_hms_opt(0, 0, 0)?;
        return now.timezone().from_local_datetime(&dt).earliest().map(|t| t.timestamp());
    }
    // Year-month shorthand: `2026-10`
    if norm.len() == 7 {
        if let Ok(nd) = NaiveDate::parse_from_str(&format!("{norm}-01"), "%Y-%m-%d") {
            let dt = nd.and_hms_opt(0, 0, 0)?;
            return now.timezone().from_local_datetime(&dt).earliest().map(|t| t.timestamp());
        }
    }
    // Year shorthand: `2025`
    if norm.len() == 4 {
        if let Ok(y) = norm.parse::<i32>() {
            if let Some(nd) = NaiveDate::from_ymd_opt(y, 1, 1) {
                let dt = nd.and_hms_opt(0, 0, 0)?;
                return now.timezone().from_local_datetime(&dt).earliest().map(|t| t.timestamp());
            }
        }
    }
    let _ = now.year();
    None
}

/// Parses a human search string into a `MailQuery`, applying `scope` unless overridden by `in:`.
pub fn parse_search_query(raw: &str, scope: &SearchScope) -> ParsedSearch {
    parse_search_query_at(raw, scope, Local::now())
}

pub fn parse_search_query_at(raw: &str, scope: &SearchScope, now: chrono::DateTime<Local>) -> ParsedSearch {
    let mut q = MailQuery::default();
    match scope {
        SearchScope::All => {}
        SearchScope::Account { account_id } => q.account_id = Some(*account_id),
        SearchScope::Unified { role } => q.role = Some(role.clone()),
        SearchScope::Folder { folder_id } => q.folder_id = Some(*folder_id),
    }

    let mut free_text: Vec<String> = Vec::new();
    let mut has_operators = false;

    for tok in tokenize(raw) {
        if let Some((k, v_raw)) = tok.split_once(':') {
            let key = k.to_ascii_lowercase();
            let val = unquote(v_raw);
            if val.is_empty() {
                // Incomplete operator while user is typing (e.g., `from:`): don't search literally for "from:"
                if matches!(
                    key.as_str(),
                    "from" | "to" | "subject" | "has" | "is" | "in" | "after" | "before" | "older_than" | "newer_than"
                ) {
                    has_operators = true;
                    continue;
                }
                free_text.push(tok);
                continue;
            }
            let val_lc = val.to_ascii_lowercase();
            match key.as_str() {
                "from" => {
                    q.from = Some(val);
                    has_operators = true;
                }
                "to" => {
                    q.to = Some(val);
                    has_operators = true;
                }
                "subject" => {
                    q.subject = Some(match q.subject {
                        Some(prev) => format!("{prev} {val}"),
                        None => val,
                    });
                    has_operators = true;
                }
                "has" => match val_lc.as_str() {
                    "attachment" | "attachments" | "file" | "files" => {
                        q.has_attachments = Some(true);
                        has_operators = true;
                    }
                    _ => free_text.push(tok),
                },
                "is" => match val_lc.as_str() {
                    "unread" => {
                        q.unread = Some(true);
                        has_operators = true;
                    }
                    "read" => {
                        q.unread = Some(false);
                        has_operators = true;
                    }
                    "flagged" | "starred" => {
                        q.flagged = Some(true);
                        has_operators = true;
                    }
                    "unflagged" | "unstarred" => {
                        q.flagged = Some(false);
                        has_operators = true;
                    }
                    _ => free_text.push(tok),
                },
                "in" => {
                    let mapped = match val_lc.as_str() {
                        "spam" => "junk",
                        "bin" => "trash",
                        "anywhere" => "all",
                        other => other,
                    };
                    if mapped == "all" {
                        q.role = None;
                        q.folder_id = None;
                        has_operators = true;
                    } else if UNIFIED_ROLES.contains(&mapped) {
                        q.role = Some(mapped.to_string());
                        q.folder_id = None;
                        has_operators = true;
                    } else {
                        free_text.push(tok);
                    }
                }
                "after" | "newer_than" => {
                    if let Some(ts) = parse_date(&val, now) {
                        q.after = Some(ts);
                        has_operators = true;
                    } else {
                        free_text.push(tok);
                    }
                }
                "before" | "older_than" => {
                    if let Some(ts) = parse_date(&val, now) {
                        q.before = Some(ts);
                        has_operators = true;
                    } else {
                        free_text.push(tok);
                    }
                }
                _ => free_text.push(tok),
            }
        } else {
            let clean = unquote(&tok);
            if !clean.is_empty() {
                free_text.push(clean);
            }
        }
    }

    if !free_text.is_empty() {
        q.text = Some(free_text.join(" "));
    }
    ParsedSearch { query: q, has_operators }
}

/// True when the parsed query has at least one condition (not just an empty incomplete operator).
pub fn has_criteria(q: &MailQuery) -> bool {
    q.text.is_some()
        || q.subject.is_some()
        || q.from.is_some()
        || q.to.is_some()
        || q.after.is_some()
        || q.before.is_some()
        || q.unread.is_some()
        || q.flagged.is_some()
        || q.has_attachments.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed_now() -> chrono::DateTime<Local> {
        Local.with_ymd_and_hms(2026, 11, 26, 12, 0, 0).unwrap()
    }

    #[test]
    fn parses_plain_and_structured_operators() {
        let p = parse_search_query_at(
            r#"from:"Maya Chen" subject:invoice has:attachment is:unread after:2026-10-01 budget"#,
            &SearchScope::All,
            fixed_now(),
        );
        assert!(p.has_operators);
        assert_eq!(p.query.from.as_deref(), Some("Maya Chen"));
        assert_eq!(p.query.subject.as_deref(), Some("invoice"));
        assert_eq!(p.query.has_attachments, Some(true));
        assert_eq!(p.query.unread, Some(true));
        assert!(p.query.after.is_some());
        assert_eq!(p.query.text.as_deref(), Some("budget"));
    }

    #[test]
    fn supports_relative_dates_and_in_scope_override() {
        let p = parse_search_query_at(
            "is:starred newer_than:7d in:sent",
            &SearchScope::Folder { folder_id: 42 },
            fixed_now(),
        );
        assert!(p.has_operators);
        assert_eq!(p.query.flagged, Some(true));
        assert_eq!(p.query.role.as_deref(), Some("sent"));
        assert_eq!(p.query.folder_id, None);
        assert_eq!(p.query.after, Some((fixed_now() - Duration::days(7)).timestamp()));
    }

    #[test]
    fn plain_text_without_operators() {
        let p = parse_search_query_at("quarterly report", &SearchScope::Unified { role: "inbox".into() }, fixed_now());
        assert!(!p.has_operators);
        assert_eq!(p.query.text.as_deref(), Some("quarterly report"));
        assert_eq!(p.query.role.as_deref(), Some("inbox"));
    }

    #[test]
    fn incomplete_operator_produces_empty_criteria() {
        let p = parse_search_query_at("from:", &SearchScope::All, fixed_now());
        assert!(p.has_operators);
        assert!(!has_criteria(&p.query));
    }
}
