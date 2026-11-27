use super::{row_to_summary, scope_sql, Db, SUMMARY_COLS};
use crate::model::*;
use anyhow::{anyhow, Result};
use rusqlite::params;

/// Turns free text into a safe FTS5 prefix query.
pub(super) fn fts_query(q: &str) -> Option<String> {
    let terms: Vec<String> = q
        .split_whitespace()
        .map(|t| t.replace(['"', '\0'], ""))
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{}\"*", t))
        .collect();
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" "))
    }
}

/// Turns a quoted phrase into a safe FTS5 exact phrase query (no trailing wildcard).
fn fts_phrase(q: &str) -> Option<String> {
    let cleaned: Vec<String> = q
        .split_whitespace()
        .map(|t| t.replace(['"', '\0'], ""))
        .filter(|t| !t.is_empty())
        .collect();
    if cleaned.is_empty() {
        None
    } else {
        Some(format!("\"{}\"", cleaned.join(" ")))
    }
}

/// A LIKE pattern matching `s` anywhere (with `ESCAPE '\'`).
pub(super) fn like_any(s: &str) -> String {
    format!("%{}%", s.trim().replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"))
}

/// Compiles a `SearchExpr` AST into a parameterized SQLite predicate over `messages m`.
fn compile_search_expr(expr: &crate::search::SearchExpr, args: &mut Vec<rusqlite::types::Value>) -> String {
    use crate::search::{SearchExpr, SearchTerm};
    use rusqlite::types::Value;
    let mut arg = |v: Value| {
        args.push(v);
        format!("?{}", args.len())
    };
    match expr {
        SearchExpr::Term(term) => match term {
            SearchTerm::Text(t) => match fts_query(t) {
                Some(fts) => {
                    let p = arg(Value::Text(fts));
                    format!("m.id IN (SELECT rowid FROM messages_fts WHERE messages_fts MATCH {p})")
                }
                None => "1".into(),
            },
            SearchTerm::Phrase(p) => match fts_phrase(p) {
                Some(fts) => {
                    let param = arg(Value::Text(fts));
                    format!("m.id IN (SELECT rowid FROM messages_fts WHERE messages_fts MATCH {param})")
                }
                None => "1".into(),
            },
            SearchTerm::Subject(s) => match fts_query(s) {
                Some(fts) => {
                    let p = arg(Value::Text(format!("subject : ({fts})")));
                    format!("m.id IN (SELECT rowid FROM messages_fts WHERE messages_fts MATCH {p})")
                }
                None => "1".into(),
            },
            SearchTerm::Body(b) => match fts_query(b) {
                Some(fts) => {
                    let p = arg(Value::Text(format!("body : ({fts})")));
                    format!("m.id IN (SELECT rowid FROM messages_fts WHERE messages_fts MATCH {p})")
                }
                None => "1".into(),
            },
            SearchTerm::From(f) => {
                let p = arg(Value::Text(like_any(f)));
                format!("(m.from_name LIKE {p} ESCAPE '\\' OR m.from_addr LIKE {p} ESCAPE '\\')")
            }
            SearchTerm::To(t) => {
                let p = arg(Value::Text(like_any(t)));
                format!(
                    "(EXISTS (SELECT 1 FROM json_each(m.to_json) j WHERE json_extract(j.value, '$.email') LIKE {p} ESCAPE '\\' OR COALESCE(json_extract(j.value, '$.name'), '') LIKE {p} ESCAPE '\\') \
                      OR EXISTS (SELECT 1 FROM json_each(m.cc_json) j WHERE json_extract(j.value, '$.email') LIKE {p} ESCAPE '\\' OR COALESCE(json_extract(j.value, '$.name'), '') LIKE {p} ESCAPE '\\'))"
                )
            }
            SearchTerm::Filename(f) => {
                let p = arg(Value::Text(like_any(f)));
                format!("m.attachment_names LIKE {p} ESCAPE '\\'")
            }
            SearchTerm::After(ts) => format!("m.date >= {}", arg(Value::Integer(*ts))),
            SearchTerm::Before(ts) => format!("m.date < {}", arg(Value::Integer(*ts))),
            SearchTerm::Larger(bytes) => format!("m.size >= {}", arg(Value::Integer(*bytes))),
            SearchTerm::Smaller(bytes) => format!("m.size < {}", arg(Value::Integer(*bytes))),
            SearchTerm::Unread(u) => format!("m.seen = {}", if *u { 0 } else { 1 }),
            SearchTerm::Flagged(f) => format!("m.flagged = {}", *f as i32),
            SearchTerm::HasAttachment(a) => format!("m.has_attachments = {}", *a as i32),
            SearchTerm::InRole(role) => {
                if role == "all" {
                    "1".into()
                } else {
                    let p = arg(Value::Text(role.clone()));
                    format!("m.folder_id IN (SELECT id FROM folders WHERE role = {p})")
                }
            }
        },
        SearchExpr::Not(inner) => format!("NOT ({})", compile_search_expr(inner, args)),
        SearchExpr::And(parts) => {
            if parts.is_empty() {
                "1".into()
            } else {
                let sqls: Vec<String> = parts.iter().map(|p| compile_search_expr(p, args)).collect();
                format!("({})", sqls.join(" AND "))
            }
        }
        SearchExpr::Or(parts) => {
            if parts.is_empty() {
                "0".into()
            } else {
                let sqls: Vec<String> = parts.iter().map(|p| compile_search_expr(p, args)).collect();
                format!("({})", sqls.join(" OR "))
            }
        }
    }
}

/// The WHERE clause (over `messages m`) and its parameters for a structured query.
fn query_where(q: &MailQuery) -> Result<(String, Vec<rusqlite::types::Value>)> {
    use rusqlite::types::Value;
    let mut args: Vec<Value> = Vec::new();
    let mut arg = |v: Value| {
        args.push(v);
        format!("?{}", args.len())
    };
    let nonempty = |s: &Option<String>| s.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);

    // Conditions on words, sender and recipients: a server hit already matched these.
    let mut words = Vec::new();
    let mut fts = Vec::new();
    if let Some(t) = q.text.as_deref().and_then(fts_query) {
        fts.push(t);
    }
    if let Some(t) = q.subject.as_deref().and_then(fts_query) {
        fts.push(format!("subject : ({t})"));
    }
    if !fts.is_empty() {
        let p = arg(Value::Text(fts.join(" AND ")));
        words.push(format!("m.id IN (SELECT rowid FROM messages_fts WHERE messages_fts MATCH {p})"));
    }
    if let Some(f) = nonempty(&q.from) {
        let p = arg(Value::Text(like_any(&f)));
        words.push(format!("(m.from_name LIKE {p} ESCAPE '\\' OR m.from_addr LIKE {p} ESCAPE '\\')"));
    }
    if let Some(t) = nonempty(&q.to) {
        let p = arg(Value::Text(like_any(&t)));
        words.push(format!(
            "(EXISTS (SELECT 1 FROM json_each(m.to_json) j WHERE json_extract(j.value, '$.email') LIKE {p} ESCAPE '\\' OR COALESCE(json_extract(j.value, '$.name'), '') LIKE {p} ESCAPE '\\') \
              OR EXISTS (SELECT 1 FROM json_each(m.cc_json) j WHERE json_extract(j.value, '$.email') LIKE {p} ESCAPE '\\' OR COALESCE(json_extract(j.value, '$.name'), '') LIKE {p} ESCAPE '\\'))"
        ));
    }

    let mut w = Vec::new();
    if !words.is_empty() {
        let all = words.join(" AND ");
        if q.server_hits.is_empty() {
            w.push(all);
        } else {
            let ids: Vec<String> = q.server_hits.iter().map(|i| i.to_string()).collect();
            w.push(format!("(({all}) OR m.id IN ({}))", ids.join(",")));
        }
    }
    if let Some(fname) = nonempty(&q.filename) {
        let p = arg(Value::Text(like_any(&fname)));
        w.push(format!("m.attachment_names LIKE {p} ESCAPE '\\'"));
    }
    if let Some(bytes) = q.larger {
        w.push(format!("m.size >= {}", arg(Value::Integer(bytes))));
    }
    if let Some(bytes) = q.smaller {
        w.push(format!("m.size < {}", arg(Value::Integer(bytes))));
    }
    if let Some(t) = q.after {
        w.push(format!("m.date >= {}", arg(Value::Integer(t))));
    }
    if let Some(t) = q.before {
        w.push(format!("m.date < {}", arg(Value::Integer(t))));
    }
    if let Some(u) = q.unread {
        w.push(format!("m.seen = {}", if u { 0 } else { 1 }));
    }
    if let Some(f) = q.flagged {
        w.push(format!("m.flagged = {}", f as i32));
    }
    if let Some(a) = q.has_attachments {
        w.push(format!("m.has_attachments = {}", a as i32));
    }
    if let Some(a) = q.account_id {
        w.push(format!("m.account_id = {}", arg(Value::Integer(a))));
    }
    if let Some(f) = q.folder_id {
        w.push(format!("m.folder_id = {}", arg(Value::Integer(f))));
    }
    if let Some(role) = nonempty(&q.role) {
        let role = role.to_ascii_lowercase();
        if role != "all" && !UNIFIED_ROLES.contains(&role.as_str()) {
            return Err(anyhow!("Unknown folder role {role:?}: use inbox, sent, drafts, archive, junk, trash, snoozed or all"));
        }
        w.push(format!("m.folder_id IN (SELECT id FROM folders WHERE role = {})", arg(Value::Text(role))));
    }
    if let Some(allowed) = &q.allowed_folders {
        if allowed.is_empty() {
            w.push("0".into());
        } else {
            let list: String = allowed.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(",");
            w.push(format!("m.folder_id IN ({list})"));
            // Also exclude messages whose Message-ID sits in a hidden folder of the same account
            // (e.g. a Spam/Trash copy also present in Gmail's All Mail).
            w.push(format!(
                "(m.message_id IS NULL OR NOT EXISTS (SELECT 1 FROM messages h WHERE h.account_id = m.account_id AND h.message_id = m.message_id AND h.folder_id NOT IN ({list})))"
            ));
        }
    }
    // The same message can sit in several folders (Gmail keeps everything in All Mail too).
    // Without a folder filter, count it once, preferring a folder other than All Mail.
    if q.folder_id.is_none() && nonempty(&q.role).is_none() {
        if let Some(allowed) = &q.allowed_folders {
            if !allowed.is_empty() {
                let list: String = allowed.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(",");
                w.push(format!(
                    "(m.message_id IS NULL OR m.id = (SELECT d.id FROM messages d JOIN folders df ON df.id = d.folder_id
                      WHERE d.account_id = m.account_id AND d.message_id = m.message_id AND d.folder_id IN ({list}) ORDER BY df.role IS 'all', d.id LIMIT 1))"
                ));
            }
        } else {
            w.push(
                "(m.message_id IS NULL OR m.id = (SELECT d.id FROM messages d JOIN folders df ON df.id = d.folder_id
                  WHERE d.account_id = m.account_id AND d.message_id = m.message_id ORDER BY df.role IS 'all', d.id LIMIT 1))"
                    .into(),
            );
        }
    }
    Ok((if w.is_empty() { "1".into() } else { w.join(" AND ") }, args))
}


impl Db {
    /// Searches the folders a scope covers, newest first.
    pub fn search(&self, query: &str, scope: &SearchScope, limit: i64) -> Result<Vec<MessageSummary>> {
        let Some(fts) = fts_query(query) else { return Ok(vec![]) };
        let filter = match scope {
            SearchScope::All => "1".to_string(),
            SearchScope::Account { account_id } => format!("m.account_id = {account_id}"),
            SearchScope::Unified { role } => scope_sql(&View::Unified { role: role.clone() })?,
            SearchScope::Folder { folder_id } => scope_sql(&View::Folder { folder_id: *folder_id })?,
        };
        let c = self.r();
        let sql = format!(
            "SELECT {SUMMARY_COLS} FROM messages m JOIN messages_fts ON messages_fts.rowid = m.id
             WHERE {filter} AND messages_fts MATCH ?1 ORDER BY m.date DESC LIMIT ?2"
        );
        let mut st = c.prepare(&sql)?;
        let rows = st.query_map(params![fts, limit], row_to_summary)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    fn search_expr_where(
        expr: &crate::search::SearchExpr,
        scope: &SearchScope,
        has_in_scope: bool,
        unread_only: bool,
    ) -> Result<(String, Vec<rusqlite::types::Value>)> {
        let mut args: Vec<rusqlite::types::Value> = Vec::new();
        let mut w: Vec<String> = Vec::new();
        match scope {
            SearchScope::All => {}
            SearchScope::Account { account_id } => {
                args.push(rusqlite::types::Value::Integer(*account_id));
                w.push(format!("m.account_id = ?{}", args.len()));
            }
            SearchScope::Unified { role } if !has_in_scope => {
                w.push(scope_sql(&View::Unified { role: role.clone() })?);
            }
            SearchScope::Folder { folder_id } if !has_in_scope => {
                w.push(scope_sql(&View::Folder { folder_id: *folder_id })?);
            }
            SearchScope::Unified { .. } | SearchScope::Folder { .. } => {}
        }
        w.push(compile_search_expr(expr, &mut args));
        if unread_only {
            w.push("m.seen = 0".into());
        }
        // Deduplicate copies across Gmail's All Mail when not pinned to a single folder/role.
        if has_in_scope || matches!(scope, SearchScope::All | SearchScope::Account { .. }) {
            w.push(
                "(m.message_id IS NULL OR m.id = (SELECT d.id FROM messages d JOIN folders df ON df.id = d.folder_id
                  WHERE d.account_id = m.account_id AND d.message_id = m.message_id ORDER BY df.role IS 'all', d.id LIMIT 1))"
                    .into(),
            );
        }
        Ok((w.join(" AND "), args))
    }

    /// Evaluates a parsed boolean search AST (`OR`, parentheses, `-negation`, phrases, `filename:`, `larger:`/`smaller:`, etc.).
    pub fn search_expr(
        &self,
        expr: &crate::search::SearchExpr,
        scope: &SearchScope,
        has_in_scope: bool,
        limit: i64,
    ) -> Result<Vec<MessageSummary>> {
        let (filter, args) = Self::search_expr_where(expr, scope, has_in_scope, false)?;
        let c = self.r();
        let sql = format!("SELECT {SUMMARY_COLS} FROM messages m WHERE {filter} ORDER BY m.date DESC, m.id DESC LIMIT {limit}");
        let mut st = c.prepare(&sql)?;
        let rows = st.query_map(rusqlite::params_from_iter(args.iter()), row_to_summary)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Counts unread messages matching a parsed boolean search AST (for Saved Search badges).
    pub fn count_search_expr_unread(&self, expr: &crate::search::SearchExpr, scope: &SearchScope, has_in_scope: bool) -> Result<i64> {
        let (filter, args) = Self::search_expr_where(expr, scope, has_in_scope, true)?;
        let c = self.r();
        let n: i64 = c.query_row(
            &format!("SELECT COUNT(*) FROM messages m WHERE {filter}"),
            rusqlite::params_from_iter(args.iter()),
            |r| r.get(0),
        )?;
        Ok(n)
    }

    /// Structured search: one page of matches, newest first (or oldest first), and the total.
    pub fn query_mail(&self, q: &MailQuery) -> Result<MailQueryResult> {
        let (filter, args) = query_where(q)?;
        let c = self.r();
        let total: i64 = c.query_row(
            &format!("SELECT COUNT(*) FROM messages m WHERE {filter}"),
            rusqlite::params_from_iter(args.iter()),
            |r| r.get(0),
        )?;
        let limit = q.limit.unwrap_or(25).clamp(1, 200);
        let offset = q.offset.unwrap_or(0).max(0);
        let order = if q.oldest_first { "ASC" } else { "DESC" };
        let sql = format!(
            "SELECT {SUMMARY_COLS} FROM messages m WHERE {filter} ORDER BY m.date {order}, m.id {order} LIMIT {limit} OFFSET {offset}"
        );
        let mut st = c.prepare(&sql)?;
        let rows = st.query_map(rusqlite::params_from_iter(args.iter()), row_to_summary)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(MailQueryResult { total, rows })
    }

    /// Counts matches per group, biggest groups first (at most 50), and the total.
    /// `by`: sender, domain, recipient, month, year, folder or account.
    pub fn group_mail(&self, q: &MailQuery, by: &str) -> Result<(i64, Vec<MailGroup>)> {
        let (filter, args) = query_where(q)?;
        let (from, key, label) = match by {
            "sender" => ("messages m", "lower(m.from_addr)", "MAX(NULLIF(m.from_name, ''))"),
            "domain" => ("messages m", "lower(substr(m.from_addr, instr(m.from_addr, '@') + 1))", "NULL"),
            "recipient" => (
                "messages m, json_each(m.to_json) j",
                "lower(json_extract(j.value, '$.email'))",
                "MAX(json_extract(j.value, '$.name'))",
            ),
            "month" => ("messages m", "strftime('%Y-%m', m.date, 'unixepoch', 'localtime')", "NULL"),
            "year" => ("messages m", "strftime('%Y', m.date, 'unixepoch', 'localtime')", "NULL"),
            "folder" => (
                "messages m",
                "CAST(m.folder_id AS TEXT)",
                "(SELECT f.name || ' · ' || a.email FROM folders f JOIN accounts a ON a.id = f.account_id WHERE f.id = m.folder_id)",
            ),
            "account" => ("messages m", "CAST(m.account_id AS TEXT)", "(SELECT a.email FROM accounts a WHERE a.id = m.account_id)"),
            _ => return Err(anyhow!("Unknown grouping {by:?}: use sender, domain, recipient, month, year, folder or account")),
        };
        let c = self.r();
        let total: i64 = c.query_row(
            &format!("SELECT COUNT(*) FROM messages m WHERE {filter}"),
            rusqlite::params_from_iter(args.iter()),
            |r| r.get(0),
        )?;
        let sql = format!(
            "SELECT {key} AS k, {label}, COUNT(*), SUM(m.seen = 0), MAX(m.date)
             FROM {from} WHERE {filter} GROUP BY k ORDER BY COUNT(*) DESC, MAX(m.date) DESC LIMIT 50"
        );
        let mut st = c.prepare(&sql)?;
        let groups = st
            .query_map(rusqlite::params_from_iter(args.iter()), |r| {
                Ok(MailGroup {
                    key: r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                    label: r.get(1)?,
                    count: r.get(2)?,
                    unread: r.get(3)?,
                    latest: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok((total, groups))
    }


    // ---------- saved searches ----------

    pub fn saved_searches(&self) -> Result<Vec<SavedSearch>> {
        let c = self.r();
        let mut s = c.prepare("SELECT id, name, query, scope_json, position FROM saved_searches ORDER BY position ASC, id ASC")?;
        let rows = s.query_map([], |r| {
            let scope_json: String = r.get(3)?;
            let scope: SearchScope = serde_json::from_str(&scope_json).unwrap_or_default();
            Ok(SavedSearch {
                id: r.get(0)?,
                name: r.get(1)?,
                query: r.get(2)?,
                scope,
                position: r.get(4)?,
                unread: 0,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn save_saved_search(&self, id: i64, name: &str, query: &str, scope: &SearchScope) -> Result<SavedSearch> {
        let name = name.trim();
        let query = query.trim();
        if name.is_empty() {
            return Err(anyhow!("Give the saved search a name"));
        }
        if query.is_empty() {
            return Err(anyhow!("Enter a search query"));
        }
        let scope_json = serde_json::to_string(scope)?;
        let c = self.c();
        let final_id = if id > 0 {
            let n = c.execute(
                "UPDATE saved_searches SET name = ?, query = ?, scope_json = ? WHERE id = ?",
                params![name, query, scope_json, id],
            )?;
            if n == 0 {
                return Err(anyhow!("Saved search {id} no longer exists"));
            }
            id
        } else {
            let pos: i64 = c.query_row("SELECT COALESCE(MAX(position), -1) + 1 FROM saved_searches", [], |r| r.get(0))?;
            c.execute(
                "INSERT INTO saved_searches (name, query, scope_json, position) VALUES (?, ?, ?, ?)",
                params![name, query, scope_json, pos],
            )?;
            c.last_insert_rowid()
        };
        let pos: i64 = c.query_row("SELECT position FROM saved_searches WHERE id = ?", [final_id], |r| r.get(0))?;
        Ok(SavedSearch {
            id: final_id,
            name: name.to_string(),
            query: query.to_string(),
            scope: scope.clone(),
            position: pos,
            unread: 0,
        })
    }

    pub fn delete_saved_search(&self, id: i64) -> Result<()> {
        self.c().execute("DELETE FROM saved_searches WHERE id = ?", [id])?;
        Ok(())
    }

    pub fn reorder_saved_searches(&self, ids: &[i64]) -> Result<()> {
        let mut c = self.c();
        let tx = c.transaction()?;
        {
            let mut st = tx.prepare_cached("UPDATE saved_searches SET position = ? WHERE id = ?")?;
            for (pos, id) in ids.iter().enumerate() {
                st.execute(params![pos as i64, id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

}
