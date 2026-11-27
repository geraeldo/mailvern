//! Human-facing search query parser: turns Gmail-style operators and boolean expressions
//! (`from:`, `to:`, `subject:`, `body:`, `filename:`, `larger:`, `smaller:`, `has:attachment`,
//! `is:unread`, `is:flagged`, `in:sent`, `after:`, `before:`, `older_than:`, `newer_than:`,
//! `"quoted phrases"`, `OR`, parentheses `(...)`, and `-negation`) into both a boolean AST
//! (`SearchExpr`) for rich local SQLite search and a best-effort flat `MailQuery` for server search.

use crate::model::{MailQuery, SearchScope, UNIFIED_ROLES};
use chrono::{Datelike, Duration, Local, NaiveDate, TimeZone};

/// One leaf predicate in a parsed search expression.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchTerm {
    /// Free-text word (prefix match across subject, sender, body).
    Text(String),
    /// Quoted exact phrase across subject, sender, body.
    Phrase(String),
    Subject(String),
    Body(String),
    From(String),
    To(String),
    Filename(String),
    After(i64),
    Before(i64),
    Larger(i64),
    Smaller(i64),
    Unread(bool),
    Flagged(bool),
    HasAttachment(bool),
    /// Standard folder role ("inbox", "sent", ..., or "all" for anywhere).
    InRole(String),
}

/// Boolean AST for local search evaluation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchExpr {
    Term(SearchTerm),
    Not(Box<SearchExpr>),
    And(Vec<SearchExpr>),
    Or(Vec<SearchExpr>),
}

/// Parsed human search query, with both the full boolean AST and a flat fallback query.
#[derive(Clone, Debug)]
pub struct ParsedSearch {
    pub query: MailQuery,
    pub expr: Option<SearchExpr>,
    pub has_operators: bool,
    pub has_boolean: bool,
    /// True if an `in:` operator appeared in the query (overrides the default folder/role scope).
    pub has_in_scope: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Tok {
    LParen,
    RParen,
    Dash,
    Or,
    Atom { text: String, quoted: bool },
}

/// Tokenizes a search string into parentheses, prefix `-` negation, `OR`, quoted phrases, and `key:val` atoms.
fn tokenize(input: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '(' {
            out.push(Tok::LParen);
            i += 1;
            continue;
        }
        if c == ')' {
            out.push(Tok::RParen);
            i += 1;
            continue;
        }
        // Leading `-` before a token or `(` is negation (not standalone `-` followed by whitespace).
        if c == '-' && i + 1 < len && !chars[i + 1].is_whitespace() && chars[i + 1] != ')' {
            out.push(Tok::Dash);
            i += 1;
            continue;
        }
        // Standalone quoted phrase: `"quarterly report"`
        if c == '"' {
            i += 1;
            let start = i;
            while i < len && chars[i] != '"' {
                i += 1;
            }
            let text: String = chars[start..i].iter().collect();
            if i < len && chars[i] == '"' {
                i += 1;
            }
            let trimmed = text.trim().to_string();
            if !trimmed.is_empty() {
                out.push(Tok::Atom { text: trimmed, quoted: true });
            }
            continue;
        }
        // Regular word or `key:"quoted value"` atom.
        let mut cur = String::new();
        let mut in_quotes = false;
        while i < len {
            let ch = chars[i];
            if ch == '"' {
                in_quotes = !in_quotes;
                cur.push(ch);
                i += 1;
            } else if !in_quotes && (ch.is_whitespace() || ch == '(' || ch == ')') {
                break;
            } else {
                cur.push(ch);
                i += 1;
            }
        }
        if cur == "OR" {
            out.push(Tok::Or);
        } else if !cur.is_empty() {
            out.push(Tok::Atom { text: cur, quoted: false });
        }
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

/// Parses a byte size token such as `5M`, `100K`, `1.5MB`, `2G`, or `4096`.
pub fn parse_size(val: &str) -> Option<i64> {
    let v = val.trim().to_ascii_uppercase();
    if v.is_empty() {
        return None;
    }
    let (num_str, mult): (&str, f64) = if let Some(s) = v.strip_suffix("GB").or_else(|| v.strip_suffix('G')) {
        (s, 1024.0 * 1024.0 * 1024.0)
    } else if let Some(s) = v.strip_suffix("MB").or_else(|| v.strip_suffix('M')) {
        (s, 1024.0 * 1024.0)
    } else if let Some(s) = v.strip_suffix("KB").or_else(|| v.strip_suffix('K')) {
        (s, 1024.0)
    } else if let Some(s) = v.strip_suffix('B') {
        (s, 1.0)
    } else {
        (v.as_str(), 1.0)
    };
    let n: f64 = num_str.trim().parse().ok()?;
    if !n.is_finite() || n < 0.0 {
        return None;
    }
    let bytes = (n * mult).round();
    if bytes > i64::MAX as f64 {
        return None;
    }
    Some(bytes as i64)
}

/// Parses a date token (`YYYY-MM-DD`, `YYYY/MM/DD`, `today`, `yesterday`, or `7d`/`2w`/`3m`/`1y`).
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
    if norm.len() == 7 {
        if let Ok(nd) = NaiveDate::parse_from_str(&format!("{norm}-01"), "%Y-%m-%d") {
            let dt = nd.and_hms_opt(0, 0, 0)?;
            return now.timezone().from_local_datetime(&dt).earliest().map(|t| t.timestamp());
        }
    }
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

enum AtomOutcome {
    Term(SearchTerm, bool /* is_operator */),
    /// Incomplete operator while typing (e.g. `from:`): counts as operator but produces no predicate.
    IncompleteOp,
}

fn has_searchable_char(s: &str) -> bool {
    s.chars().any(|c| c.is_alphanumeric() || matches!(c, '×' | '✕' | '✖'))
}

fn parse_atom(text: &str, quoted: bool, now: chrono::DateTime<Local>) -> Option<AtomOutcome> {
    if quoted {
        let t = text.trim();
        return has_searchable_char(t).then(|| AtomOutcome::Term(SearchTerm::Phrase(t.to_string()), false));
    }
    if let Some((k, v_raw)) = text.split_once(':') {
        let key = k.to_ascii_lowercase();
        let val = unquote(v_raw);
        let known_key = matches!(
            key.as_str(),
            "from"
                | "to"
                | "subject"
                | "body"
                | "filename"
                | "has"
                | "is"
                | "in"
                | "after"
                | "before"
                | "older_than"
                | "newer_than"
                | "larger"
                | "smaller"
        );
        if val.is_empty() {
            if known_key {
                return Some(AtomOutcome::IncompleteOp);
            }
            let clean = unquote(text);
            return has_searchable_char(&clean).then_some(AtomOutcome::Term(SearchTerm::Text(clean), false));
        }
        let val_lc = val.to_ascii_lowercase();
        match key.as_str() {
            "from" => return Some(AtomOutcome::Term(SearchTerm::From(val), true)),
            "to" => return Some(AtomOutcome::Term(SearchTerm::To(val), true)),
            "subject" => return Some(AtomOutcome::Term(SearchTerm::Subject(val), true)),
            "body" => return Some(AtomOutcome::Term(SearchTerm::Body(val), true)),
            "filename" => return Some(AtomOutcome::Term(SearchTerm::Filename(val), true)),
            "has" => match val_lc.as_str() {
                "attachment" | "attachments" | "file" | "files" => {
                    return Some(AtomOutcome::Term(SearchTerm::HasAttachment(true), true));
                }
                _ => {}
            },
            "is" => match val_lc.as_str() {
                "unread" => return Some(AtomOutcome::Term(SearchTerm::Unread(true), true)),
                "read" => return Some(AtomOutcome::Term(SearchTerm::Unread(false), true)),
                "flagged" | "starred" => return Some(AtomOutcome::Term(SearchTerm::Flagged(true), true)),
                "unflagged" | "unstarred" => return Some(AtomOutcome::Term(SearchTerm::Flagged(false), true)),
                _ => {}
            },
            "in" => {
                let mapped = match val_lc.as_str() {
                    "spam" => "junk",
                    "bin" => "trash",
                    "anywhere" => "all",
                    other => other,
                };
                if mapped == "all" || UNIFIED_ROLES.contains(&mapped) {
                    return Some(AtomOutcome::Term(SearchTerm::InRole(mapped.to_string()), true));
                }
            }
            "after" | "newer_than" => {
                if let Some(ts) = parse_date(&val, now) {
                    return Some(AtomOutcome::Term(SearchTerm::After(ts), true));
                }
            }
            "before" | "older_than" => {
                if let Some(ts) = parse_date(&val, now) {
                    return Some(AtomOutcome::Term(SearchTerm::Before(ts), true));
                }
            }
            "larger" => {
                if let Some(bytes) = parse_size(&val) {
                    return Some(AtomOutcome::Term(SearchTerm::Larger(bytes), true));
                }
            }
            "smaller" => {
                if let Some(bytes) = parse_size(&val) {
                    return Some(AtomOutcome::Term(SearchTerm::Smaller(bytes), true));
                }
            }
            _ => {}
        }
    }
    let clean = unquote(text);
    has_searchable_char(&clean).then_some(AtomOutcome::Term(SearchTerm::Text(clean), false))
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
    now: chrono::DateTime<Local>,
    has_operators: bool,
    has_boolean: bool,
    has_in_scope: bool,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned()?;
        self.pos += 1;
        Some(t)
    }

    /// `and_expr := or_expr (or_expr)*`
    fn parse_and(&mut self, stop_on_rparen: bool) -> Option<SearchExpr> {
        let mut items = Vec::new();
        while let Some(tok) = self.peek() {
            if matches!(tok, Tok::RParen) {
                if stop_on_rparen {
                    break;
                }
                // Unmatched `)`: ignore and continue.
                self.next();
                continue;
            }
            if matches!(tok, Tok::Or) {
                // Stray leading/repeated `OR`: skip.
                self.next();
                self.has_boolean = true;
                continue;
            }
            if let Some(e) = self.parse_or() {
                items.push(e);
            }
        }
        match items.len() {
            0 => None,
            1 => items.into_iter().next(),
            _ => Some(SearchExpr::And(items)),
        }
    }

    /// `or_expr := unary ('OR' unary)*` (Gmail precedence: `OR` binds tighter than space-AND).
    fn parse_or(&mut self) -> Option<SearchExpr> {
        let first = self.parse_unary()?;
        let mut alts = vec![first];
        while matches!(self.peek(), Some(Tok::Or)) {
            self.next();
            self.has_boolean = true;
            if let Some(rhs) = self.parse_unary() {
                alts.push(rhs);
            }
        }
        if alts.len() == 1 {
            alts.into_iter().next()
        } else {
            Some(SearchExpr::Or(alts))
        }
    }

    /// `unary := '-' unary | primary`
    fn parse_unary(&mut self) -> Option<SearchExpr> {
        if matches!(self.peek(), Some(Tok::Dash)) {
            self.next();
            self.has_boolean = true;
            return self.parse_unary().map(|inner| SearchExpr::Not(Box::new(inner)));
        }
        self.parse_primary()
    }

    /// `primary := '(' expr ')' | atom`
    fn parse_primary(&mut self) -> Option<SearchExpr> {
        match self.next()? {
            Tok::LParen => {
                self.has_boolean = true;
                let inner = self.parse_and(true);
                if matches!(self.peek(), Some(Tok::RParen)) {
                    self.next();
                }
                inner
            }
            Tok::Atom { text, quoted } => {
                if quoted {
                    self.has_boolean = true;
                }
                match parse_atom(&text, quoted, self.now)? {
                    AtomOutcome::IncompleteOp => {
                        self.has_operators = true;
                        None
                    }
                    AtomOutcome::Term(term, is_op) => {
                        if is_op {
                            self.has_operators = true;
                        }
                        if matches!(term, SearchTerm::InRole(_)) {
                            self.has_in_scope = true;
                        }
                        Some(SearchExpr::Term(term))
                    }
                }
            }
            Tok::Dash => {
                self.has_boolean = true;
                self.parse_unary().map(|inner| SearchExpr::Not(Box::new(inner)))
            }
            Tok::Or | Tok::RParen => None,
        }
    }
}

/// Fills a best-effort flat `MailQuery` from positive top-level terms (for IMAP server search & compatibility).
fn flatten_to_query(expr: &SearchExpr, q: &mut MailQuery, free_text: &mut Vec<String>) {
    match expr {
        SearchExpr::And(parts) => {
            for p in parts {
                flatten_to_query(p, q, free_text);
            }
        }
        SearchExpr::Term(term) => match term {
            SearchTerm::Text(s) | SearchTerm::Phrase(s) => free_text.push(s.clone()),
            SearchTerm::Subject(s) => {
                q.subject = Some(match q.subject.take() {
                    Some(prev) => format!("{prev} {s}"),
                    None => s.clone(),
                });
            }
            SearchTerm::Body(s) => free_text.push(s.clone()),
            SearchTerm::From(s) => q.from = Some(s.clone()),
            SearchTerm::To(s) => q.to = Some(s.clone()),
            SearchTerm::Filename(s) => q.filename = Some(s.clone()),
            SearchTerm::After(ts) => q.after = Some(*ts),
            SearchTerm::Before(ts) => q.before = Some(*ts),
            SearchTerm::Larger(b) => q.larger = Some(*b),
            SearchTerm::Smaller(b) => q.smaller = Some(*b),
            SearchTerm::Unread(u) => q.unread = Some(*u),
            SearchTerm::Flagged(f) => q.flagged = Some(*f),
            SearchTerm::HasAttachment(a) => q.has_attachments = Some(*a),
            SearchTerm::InRole(r) => {
                if r == "all" {
                    q.role = None;
                    q.folder_id = None;
                } else {
                    q.role = Some(r.clone());
                    q.folder_id = None;
                }
            }
        },
        // Negations and OR disjunctions are handled faithfully by `SearchExpr` locally; omit from flat IMAP query.
        SearchExpr::Not(_) | SearchExpr::Or(_) => {}
    }
}

/// Parses a human search string into a `ParsedSearch`, applying `scope` unless overridden by `in:`.
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

    let mut parser = Parser {
        toks: tokenize(raw),
        pos: 0,
        now,
        has_operators: false,
        has_boolean: false,
        has_in_scope: false,
    };
    let expr = parser.parse_and(false);
    let mut free_text = Vec::new();
    if let Some(ref e) = expr {
        flatten_to_query(e, &mut q, &mut free_text);
    }
    if !free_text.is_empty() {
        q.text = Some(free_text.join(" "));
    }
    ParsedSearch {
        query: q,
        expr,
        has_operators: parser.has_operators,
        has_boolean: parser.has_boolean,
        has_in_scope: parser.has_in_scope,
    }
}

/// True when the parsed query has at least one condition (not just an empty incomplete operator).
pub fn has_criteria(q: &MailQuery) -> bool {
    q.text.is_some()
        || q.subject.is_some()
        || q.from.is_some()
        || q.to.is_some()
        || q.filename.is_some()
        || q.larger.is_some()
        || q.smaller.is_some()
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
        assert!(p.has_in_scope);
        assert_eq!(p.query.flagged, Some(true));
        assert_eq!(p.query.role.as_deref(), Some("sent"));
        assert_eq!(p.query.folder_id, None);
        assert_eq!(p.query.after, Some((fixed_now() - Duration::days(7)).timestamp()));
    }

    #[test]
    fn plain_text_without_operators() {
        let p = parse_search_query_at("quarterly report", &SearchScope::Unified { role: "inbox".into() }, fixed_now());
        assert!(!p.has_operators);
        assert!(!p.has_boolean);
        assert_eq!(p.query.text.as_deref(), Some("quarterly report"));
        assert_eq!(p.query.role.as_deref(), Some("inbox"));
    }

    #[test]
    fn incomplete_operator_produces_empty_criteria() {
        let p = parse_search_query_at("from:", &SearchScope::All, fixed_now());
        assert!(p.has_operators);
        assert!(!has_criteria(&p.query));
        assert!(p.expr.is_none());
    }

    #[test]
    fn parses_or_parentheses_negation_phrases_filename_and_sizes() {
        let p = parse_search_query_at(
            r#"(from:ana OR from:ben) -from:billing -(draft OR old) "quarterly report" filename:pdf larger:5M smaller:100K"#,
            &SearchScope::All,
            fixed_now(),
        );
        assert!(p.has_operators);
        assert!(p.has_boolean);
        assert_eq!(p.query.filename.as_deref(), Some("pdf"));
        assert_eq!(p.query.larger, Some(5 * 1024 * 1024));
        assert_eq!(p.query.smaller, Some(100 * 1024));
        assert_eq!(
            p.expr,
            Some(SearchExpr::And(vec![
                SearchExpr::Or(vec![
                    SearchExpr::Term(SearchTerm::From("ana".into())),
                    SearchExpr::Term(SearchTerm::From("ben".into())),
                ]),
                SearchExpr::Not(Box::new(SearchExpr::Term(SearchTerm::From("billing".into())))),
                SearchExpr::Not(Box::new(SearchExpr::Or(vec![
                    SearchExpr::Term(SearchTerm::Text("draft".into())),
                    SearchExpr::Term(SearchTerm::Text("old".into())),
                ]))),
                SearchExpr::Term(SearchTerm::Phrase("quarterly report".into())),
                SearchExpr::Term(SearchTerm::Filename("pdf".into())),
                SearchExpr::Term(SearchTerm::Larger(5 * 1024 * 1024)),
                SearchExpr::Term(SearchTerm::Smaller(100 * 1024)),
            ]))
        );
    }

    #[test]
    fn or_binds_tighter_than_implicit_and() {
        let p = parse_search_query_at("invoice OR receipt 2026", &SearchScope::All, fixed_now());
        assert_eq!(
            p.expr,
            Some(SearchExpr::And(vec![
                SearchExpr::Or(vec![
                    SearchExpr::Term(SearchTerm::Text("invoice".into())),
                    SearchExpr::Term(SearchTerm::Text("receipt".into())),
                ]),
                SearchExpr::Term(SearchTerm::Text("2026".into())),
            ]))
        );
    }

    #[test]
    fn parses_sizes() {
        assert_eq!(parse_size("5M"), Some(5 * 1024 * 1024));
        assert_eq!(parse_size("100k"), Some(100 * 1024));
        assert_eq!(parse_size("1.5MB"), Some((1.5 * 1024.0 * 1024.0) as i64));
        assert_eq!(parse_size("2G"), Some(2 * 1024 * 1024 * 1024));
        assert_eq!(parse_size("512"), Some(512));
        assert_eq!(parse_size("-5M"), None);
    }

    #[test]
    fn drops_standalone_punctuation_and_keeps_cross_lookalikes() {
        let p = parse_search_query_at("Quotation - AQUA x MPL S18", &SearchScope::All, fixed_now());
        assert_eq!(
            p.expr,
            Some(SearchExpr::And(vec![
                SearchExpr::Term(SearchTerm::Text("Quotation".into())),
                SearchExpr::Term(SearchTerm::Text("AQUA".into())),
                SearchExpr::Term(SearchTerm::Text("x".into())),
                SearchExpr::Term(SearchTerm::Text("MPL".into())),
                SearchExpr::Term(SearchTerm::Text("S18".into())),
            ]))
        );
        assert_eq!(p.query.text.as_deref(), Some("Quotation AQUA x MPL S18"));

        let p2 = parse_search_query_at("AQUA × MPL / S18 & 2026", &SearchScope::All, fixed_now());
        assert_eq!(p2.query.text.as_deref(), Some("AQUA × MPL S18 2026"));

        let p3 = parse_search_query_at("--", &SearchScope::All, fixed_now());
        assert!(p3.expr.is_none());
    }
}
