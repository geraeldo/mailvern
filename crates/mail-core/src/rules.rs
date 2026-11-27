//! Mail rules: which messages a rule matches, whether it may send mail about them, and
//! the mail it sends. Pure functions; `Engine` runs them on new mail.

use crate::model::*;
use crate::text::{escape_html, html_to_text, quotable_html};
use chrono::{Datelike, Duration, Local, TimeZone};
use regex::{Regex, RegexBuilder};
use serde::Serialize;

/// What rules can see of a stored message: its headers, never its body.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Facts {
    pub id: i64,
    pub account_id: i64,
    pub folder_id: i64,
    pub from_name: String,
    pub from_addr: String,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
    pub reply_to: Vec<Addr>,
    pub subject: String,
    pub list_id: Option<String>,
    pub auto: Option<String>,
    pub has_attachments: bool,
    pub has_body: bool,
    pub seen: bool,
    pub date: i64,
    pub message_id: Option<String>,
    pub refs: Option<String>,
    pub role: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Yes,
    No,
    /// The answer depends on a body condition and the body isn't here.
    NeedBody,
}

enum Test {
    /// Lowercased.
    Text(String),
    Re(Regex),
}

struct Cond {
    field: RuleField,
    op: RuleOp,
    test: Option<Test>,
}

/// A rule's conditions, ready to check many messages.
pub struct Compiled {
    match_all: bool,
    conds: Vec<Cond>,
    except: Vec<Cond>,
    /// My addresses, lowercased.
    mine: Vec<String>,
}

fn field_name(f: RuleField) -> &'static str {
    match f {
        RuleField::From => "From",
        RuleField::FromDomain => "Sender's domain",
        RuleField::To => "To",
        RuleField::Cc => "Cc",
        RuleField::ToOrCc => "To or Cc",
        RuleField::OnlyToMe => "Sent only to me",
        RuleField::CcMe => "I'm in Cc",
        RuleField::Subject => "Subject",
        RuleField::Body => "Body",
        RuleField::ListId => "Mailing list",
        RuleField::HasAttachment => "Has attachment",
    }
}

fn compile_cond(c: &RuleCondition) -> Result<Cond, String> {
    if c.field.is_flag() {
        return Ok(Cond { field: c.field, op: c.op, test: None });
    }
    let value = c.value.trim();
    if value.is_empty() {
        return Err(format!("“{}” needs something to look for", field_name(c.field)));
    }
    let test = if c.op == RuleOp::Regex {
        let re = RegexBuilder::new(value)
            .case_insensitive(true)
            .size_limit(1 << 20)
            .build()
            .map_err(|e| format!("“{value}” isn't a valid pattern: {}", e.to_string().lines().last().unwrap_or("").trim()))?;
        Test::Re(re)
    } else {
        Test::Text(value.to_lowercase())
    };
    Ok(Cond { field: c.field, op: c.op, test: Some(test) })
}

fn text_match(op: RuleOp, test: &Test, hay: &str) -> bool {
    match test {
        Test::Re(r) => r.is_match(hay),
        Test::Text(v) => {
            let h = hay.trim().to_lowercase();
            match op {
                RuleOp::Contains | RuleOp::Regex => h.contains(v.as_str()),
                RuleOp::Is => h == *v,
                RuleOp::StartsWith => h.starts_with(v.as_str()),
                RuleOp::EndsWith => h.ends_with(v.as_str()),
            }
        }
    }
}

fn addr_match(op: RuleOp, test: &Test, a: &Addr) -> bool {
    text_match(op, test, &a.email) || a.name.as_deref().is_some_and(|n| text_match(op, test, n))
}

fn domain_of(addr: &str) -> &str {
    addr.rsplit_once('@').map(|x| x.1).unwrap_or("")
}

impl Compiled {
    pub fn new(rule: &Rule, mine: &[String]) -> Result<Self, String> {
        Ok(Compiled {
            match_all: rule.match_all,
            conds: rule.conditions.iter().map(compile_cond).collect::<Result<_, _>>()?,
            except: rule.exceptions.iter().map(compile_cond).collect::<Result<_, _>>()?,
            mine: mine.iter().map(|m| m.to_lowercase()).collect(),
        })
    }

    fn is_mine(&self, addr: &str) -> bool {
        self.mine.iter().any(|m| m.eq_ignore_ascii_case(addr))
    }

    /// None: it's about the body and there is none.
    fn eval(&self, c: &Cond, f: &Facts, body: Option<&str>) -> Option<bool> {
        let t = || c.test.as_ref().expect("value conditions are compiled with a test");
        Some(match c.field {
            RuleField::From => addr_match(c.op, t(), &Addr { name: Some(f.from_name.clone()), email: f.from_addr.clone() }),
            RuleField::FromDomain => {
                let d = domain_of(&f.from_addr);
                match (c.op, t()) {
                    // "github.com" also covers "notifications.github.com".
                    (RuleOp::Is, Test::Text(v)) => {
                        let d = d.to_lowercase();
                        d == *v || d.ends_with(&format!(".{v}"))
                    }
                    (op, test) => text_match(op, test, d),
                }
            }
            RuleField::To => f.to.iter().any(|a| addr_match(c.op, t(), a)),
            RuleField::Cc => f.cc.iter().any(|a| addr_match(c.op, t(), a)),
            RuleField::ToOrCc => f.to.iter().chain(&f.cc).any(|a| addr_match(c.op, t(), a)),
            RuleField::OnlyToMe => f.to.len() == 1 && f.cc.is_empty() && self.is_mine(&f.to[0].email),
            RuleField::CcMe => f.cc.iter().any(|a| self.is_mine(&a.email)),
            RuleField::Subject => text_match(c.op, t(), &f.subject),
            RuleField::Body => return body.map(|b| text_match(c.op, t(), b)),
            RuleField::ListId => f.list_id.as_deref().is_some_and(|l| text_match(c.op, t(), l)),
            RuleField::HasAttachment => f.has_attachments,
        })
    }

    pub fn check(&self, f: &Facts, body: Option<&str>) -> Verdict {
        let vals: Vec<Option<bool>> = self.conds.iter().map(|c| self.eval(c, f, body)).collect();
        let positive = if vals.is_empty() {
            Some(true)
        } else if self.match_all {
            if vals.contains(&Some(false)) {
                Some(false)
            } else if vals.iter().all(|v| *v == Some(true)) {
                Some(true)
            } else {
                None
            }
        } else if vals.contains(&Some(true)) {
            Some(true)
        } else if vals.iter().all(|v| *v == Some(false)) {
            Some(false)
        } else {
            None
        };
        if positive == Some(false) {
            return Verdict::No;
        }
        let ex: Vec<Option<bool>> = self.except.iter().map(|c| self.eval(c, f, body)).collect();
        if ex.contains(&Some(true)) {
            return Verdict::No;
        }
        if positive == Some(true) && ex.iter().all(|v| *v == Some(false)) {
            Verdict::Yes
        } else {
            Verdict::NeedBody
        }
    }
}

/// Checks a rule before it's saved. `folders` are every account's folders.
pub fn validate(rule: &Rule, folders: &[Folder], mine: &[String]) -> Result<(), String> {
    if rule.name.trim().is_empty() {
        return Err("Give the rule a name".into());
    }
    if rule.actions.is_empty() {
        return Err("Add at least one action".into());
    }
    Compiled::new(rule, mine)?;
    if rule.actions.iter().filter(|a| a.relocates()).count() > 1 {
        return Err("A rule can move a message to one place only: keep one of Move, Archive, Delete, Junk and Snooze".into());
    }
    for a in &rule.actions {
        match a {
            RuleAction::Move { folder_id } => {
                let Some(account_id) = rule.account_id else {
                    return Err("Choose an account: a rule that moves mail to a folder works in one account".into());
                };
                let Some(f) = folders.iter().find(|f| f.id == *folder_id) else {
                    return Err("The folder to move to no longer exists".into());
                };
                if f.account_id != account_id {
                    return Err(format!("“{}” is in another account than the rule", f.name));
                }
            }
            RuleAction::Forward { to } => {
                if to.iter().all(|t| t.trim().is_empty()) {
                    return Err("Add an address to forward to".into());
                }
                for t in to.iter().map(|t| t.trim()).filter(|t| !t.is_empty()) {
                    let mb: lettre::message::Mailbox = t.parse().map_err(|_| format!("“{t}” isn't an email address"))?;
                    if mine.iter().any(|m| m.eq_ignore_ascii_case(mb.email.as_ref())) {
                        return Err(format!("Forwarding to your own address {t} could loop: pick another address"));
                    }
                }
            }
            RuleAction::Reply { text, .. } if text.trim().is_empty() => {
                return Err("Write the auto-reply".into());
            }
            _ => {}
        }
    }
    Ok(())
}

fn local_part(addr: &str) -> String {
    addr.split('@').next().unwrap_or("").to_lowercase()
}

fn check_reply_target(addr: &str, mine: &[String]) -> Result<(), &'static str> {
    let addr = addr.trim();
    if addr.is_empty() {
        return Err("it has no sender");
    }
    let local = local_part(addr);
    let machine = ["noreply", "no-reply", "donotreply", "do-not-reply", "do_not_reply"].iter().any(|n| local.contains(n))
        || ["mailer-daemon", "postmaster", "bounce", "bounces"].contains(&local.as_str());
    if machine {
        return Err("the sender doesn't take replies");
    }
    if mine.iter().any(|m| m.eq_ignore_ascii_case(addr)) {
        return Err("it's from you");
    }
    Ok(())
}

/// Whether a rule may auto-reply to this message (RFC 3834): never to machines, lists, or yourself.
pub fn may_reply(f: &Facts, mine: &[String]) -> Result<(), &'static str> {
    if f.auto.is_some() {
        return Err("it was sent automatically");
    }
    if f.list_id.is_some() {
        return Err("it came from a mailing list");
    }
    check_reply_target(&f.from_addr, mine)?;
    if let Some(reply_to) = f.reply_to.first() {
        check_reply_target(&reply_to.email, mine)?;
    }
    Ok(())
}

/// Whether a rule may forward this message: not one another rule already forwarded, which could loop.
pub fn may_forward(f: &Facts) -> Result<(), &'static str> {
    if f.auto.as_deref() == Some(AUTO_FORWARDED) {
        return Err("a rule forwarded it already");
    }
    Ok(())
}

/// `Auto-Submitted` values for mail rules send.
pub const AUTO_REPLIED: &str = "auto-replied";
pub const AUTO_FORWARDED: &str = "auto-forwarded";

/// When a snooze ends, in unix seconds.
pub fn snooze_until<Tz: TimeZone>(until: SnoozeUntil, now: chrono::DateTime<Tz>) -> i64 {
    let at_eight = |days: i64| {
        let d = now.date_naive() + Duration::days(days);
        let t = d.and_hms_opt(8, 0, 0).expect("8:00 exists");
        now.timezone().from_local_datetime(&t).earliest().map(|t| t.timestamp()).unwrap_or(now.timestamp() + days * 86_400)
    };
    match until {
        SnoozeUntil::LaterToday => now.timestamp() + 3 * 3600,
        SnoozeUntil::Tomorrow => at_eight(1),
        SnoozeUntil::NextWeek => at_eight(7 - now.weekday().num_days_from_monday() as i64),
    }
}

fn prefixed(prefix: &str, subject: &str) -> String {
    let s = subject.trim();
    if s.to_lowercase().starts_with(&format!("{}:", prefix.to_lowercase())) {
        s.to_string()
    } else {
        format!("{prefix}: {s}")
    }
}

fn quote_display_name(name: &str) -> String {
    let clean: String = name.chars().filter(|c| !matches!(c, '\r' | '\n' | '\0')).collect();
    let needs_quotes = clean
        .chars()
        .any(|c| matches!(c, '"' | '\\' | ',' | '.' | '(' | ')' | '<' | '>' | '@' | ';' | ':' | '[' | ']'));
    if needs_quotes {
        let escaped = clean.replace('\\', "\\\\").replace('"', "\\\"");
        format!("\"{escaped}\"")
    } else {
        clean
    }
}

fn who(a: &Addr) -> String {
    match a.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => format!("{} <{}>", quote_display_name(n), a.email),
        None => a.email.clone(),
    }
}

/// A forward of `m` to `to`, with its attachments (`files`, already written to disk).
pub fn forward_outgoing(m: &MessageDetail, account_id: i64, to: &[String], files: Vec<String>) -> Outgoing {
    let s = &m.summary;
    let from = who(&Addr { name: Some(s.from_name.clone()), email: s.from_addr.clone() });
    let date = Local.timestamp_opt(s.date, 0).single().map(|d| d.format("%a, %-d %b %Y at %H:%M").to_string()).unwrap_or_default();
    let list = |v: &[Addr]| v.iter().map(who).collect::<Vec<_>>().join(", ");
    let mut lines = vec![format!("From: {from}"), format!("Date: {date}"), format!("Subject: {}", s.subject), format!("To: {}", list(&m.to))];
    if !m.cc.is_empty() {
        lines.push(format!("Cc: {}", list(&m.cc)));
    }
    let head = "---------- Forwarded message ----------";
    let body_html = match (&m.html, &m.text) {
        (Some(h), _) => quotable_html(h),
        (None, Some(t)) => format!("<div style=\"white-space: pre-wrap\">{}</div>", escape_html(t)),
        (None, None) => String::new(),
    };
    let body_text = match (&m.text, &m.html) {
        (Some(t), _) => t.clone(),
        (None, Some(h)) => html_to_text(h),
        (None, None) => String::new(),
    };
    let html = format!(
        "<div>{head}<br>{}</div><br>{body_html}",
        lines.iter().map(|l| escape_html(l)).collect::<Vec<_>>().join("<br>")
    );
    let text = format!("{head}\n{}\n\n{body_text}", lines.join("\n"));
    Outgoing {
        account_id,
        to: to.iter().map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect(),
        cc: vec![],
        bcc: vec![],
        subject: prefixed("Fwd", &s.subject),
        html,
        text,
        attachments: files,
        in_reply_to: None,
        references: None,
        message_id: None,
        auto_submitted: Some(AUTO_FORWARDED.into()),
        ..Default::default()
    }
}

/// An auto-reply to `f`: to its Reply-To, else its sender.
pub fn reply_outgoing(f: &Facts, account_id: i64, subject: Option<&str>, text: &str) -> Outgoing {
    let to = f.reply_to.first().map(who).unwrap_or_else(|| who(&Addr { name: Some(f.from_name.clone()), email: f.from_addr.clone() }));
    let subject = subject.map(str::trim).filter(|s| !s.is_empty()).map(String::from).unwrap_or_else(|| prefixed("Re", &f.subject));
    let html = text.trim().split('\n').map(escape_html).collect::<Vec<_>>().join("<br>");
    let references = match (&f.refs, &f.message_id) {
        (Some(r), Some(m)) => Some(format!("{r} {m}")),
        (None, Some(m)) => Some(m.clone()),
        (r, None) => r.clone(),
    };
    Outgoing {
        account_id,
        to: vec![to],
        cc: vec![],
        bcc: vec![],
        subject,
        html: format!("<div>{html}</div>"),
        text: text.trim().to_string(),
        attachments: vec![],
        in_reply_to: f.message_id.clone(),
        references,
        message_id: None,
        auto_submitted: Some(AUTO_REPLIED.into()),
        ..Default::default()
    }
}

/// Who an auto-reply goes to, for "once per sender".
pub fn reply_address(f: &Facts) -> String {
    f.reply_to.first().map(|a| a.email.clone()).unwrap_or_else(|| f.from_addr.clone()).to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(e: &str) -> Addr {
        Addr { name: None, email: e.into() }
    }

    fn facts() -> Facts {
        Facts {
            from_name: "GitHub".into(),
            from_addr: "notifications@github.com".into(),
            to: vec![addr("me@example.com")],
            subject: "[mailvern] PR #12: Add rules".into(),
            list_id: Some("mailvern <mailvern.geraeldo.github.com>".into()),
            ..Default::default()
        }
    }

    fn cond(field: RuleField, op: RuleOp, value: &str) -> RuleCondition {
        RuleCondition { field, op, value: value.into() }
    }

    fn rule(conditions: Vec<RuleCondition>) -> Rule {
        Rule {
            id: 1,
            name: "Test".into(),
            enabled: true,
            account_id: None,
            match_all: true,
            conditions,
            exceptions: vec![],
            actions: vec![RuleAction::Archive],
            stop: false,
            origin: "user".into(),
            needs_approval: false,
            paused_reason: None,
        }
    }

    fn mine() -> Vec<String> {
        vec!["me@example.com".into()]
    }

    fn check(r: &Rule, f: &Facts, body: Option<&str>) -> Verdict {
        Compiled::new(r, &mine()).unwrap().check(f, body)
    }

    #[test]
    fn fields_and_operators() {
        let f = facts();
        let yes = |c: RuleCondition| check(&rule(vec![c]), &f, None);
        assert_eq!(yes(cond(RuleField::From, RuleOp::Contains, "GITHUB")), Verdict::Yes);
        assert_eq!(yes(cond(RuleField::From, RuleOp::Is, "notifications@github.com")), Verdict::Yes);
        assert_eq!(yes(cond(RuleField::From, RuleOp::Is, "github.com")), Verdict::No);
        assert_eq!(yes(cond(RuleField::FromDomain, RuleOp::Is, "github.com")), Verdict::Yes);
        assert_eq!(yes(cond(RuleField::FromDomain, RuleOp::Is, "hub.com")), Verdict::No);
        assert_eq!(yes(cond(RuleField::Subject, RuleOp::StartsWith, "[mailvern]")), Verdict::Yes);
        assert_eq!(yes(cond(RuleField::Subject, RuleOp::EndsWith, "add RULES")), Verdict::Yes);
        assert_eq!(yes(cond(RuleField::Subject, RuleOp::Regex, r"pr #\d+")), Verdict::Yes);
        assert_eq!(yes(cond(RuleField::ListId, RuleOp::Contains, "mailvern.geraeldo")), Verdict::Yes);
        assert_eq!(yes(cond(RuleField::To, RuleOp::Contains, "me@")), Verdict::Yes);
        assert_eq!(yes(cond(RuleField::Cc, RuleOp::Contains, "me@")), Verdict::No);
        assert_eq!(yes(cond(RuleField::OnlyToMe, RuleOp::Contains, "")), Verdict::Yes);
        assert_eq!(yes(cond(RuleField::CcMe, RuleOp::Contains, "")), Verdict::No);
        assert_eq!(yes(cond(RuleField::HasAttachment, RuleOp::Contains, "")), Verdict::No);
    }

    #[test]
    fn body_conditions_wait_for_the_body_only_when_it_matters() {
        let f = facts();
        let body = cond(RuleField::Body, RuleOp::Contains, "invoice");
        let r = rule(vec![cond(RuleField::From, RuleOp::Contains, "github"), body.clone()]);
        assert_eq!(check(&r, &f, None), Verdict::NeedBody);
        assert_eq!(check(&r, &f, Some("Your INVOICE is ready")), Verdict::Yes);
        assert_eq!(check(&r, &f, Some("hello")), Verdict::No);
        // A header that already fails decides it without the body.
        let r = rule(vec![cond(RuleField::From, RuleOp::Contains, "gitlab"), body]);
        assert_eq!(check(&r, &f, None), Verdict::No);
    }

    #[test]
    fn any_all_and_exceptions() {
        let f = facts();
        let mut r = rule(vec![cond(RuleField::From, RuleOp::Contains, "gitlab"), cond(RuleField::Subject, RuleOp::Contains, "PR")]);
        assert_eq!(check(&r, &f, None), Verdict::No);
        r.match_all = false;
        assert_eq!(check(&r, &f, None), Verdict::Yes);
        r.exceptions = vec![cond(RuleField::Subject, RuleOp::Contains, "add rules")];
        assert_eq!(check(&r, &f, None), Verdict::No);
        r.exceptions = vec![cond(RuleField::Body, RuleOp::Contains, "urgent")];
        assert_eq!(check(&r, &f, None), Verdict::NeedBody);
        assert_eq!(check(&r, &f, Some("not now")), Verdict::Yes);
        // No conditions: every message.
        assert_eq!(check(&rule(vec![]), &f, None), Verdict::Yes);
    }

    #[test]
    fn validation() {
        let folders = vec![Folder { id: 7, account_id: 1, path: "Dev".into(), name: "Dev".into(), role: None, unread: 0, position: None }];
        let ok = |r: &Rule| validate(r, &folders, &mine());
        assert!(ok(&rule(vec![cond(RuleField::Subject, RuleOp::Regex, "(unclosed")])).unwrap_err().contains("isn't a valid pattern"));
        assert!(ok(&rule(vec![cond(RuleField::Subject, RuleOp::Contains, "  ")])).is_err());
        let mut r = rule(vec![]);
        r.actions = vec![RuleAction::Move { folder_id: 7 }];
        assert!(ok(&r).unwrap_err().contains("Choose an account"));
        r.account_id = Some(2);
        assert!(ok(&r).unwrap_err().contains("another account"));
        r.account_id = Some(1);
        assert!(ok(&r).is_ok());
        r.actions.push(RuleAction::Archive);
        assert!(ok(&r).is_err());
        r.actions = vec![RuleAction::Forward { to: vec!["Me <ME@example.com>".into()] }];
        assert!(ok(&r).unwrap_err().contains("loop"));
        r.actions = vec![RuleAction::Forward { to: vec!["not an address".into()] }];
        assert!(ok(&r).is_err());
        r.actions = vec![RuleAction::Forward { to: vec!["Pat <pat@example.org>".into()] }];
        assert!(ok(&r).is_ok());
        r.actions = vec![RuleAction::Reply { subject: None, text: " ".into() }];
        assert!(ok(&r).is_err());
    }

    #[test]
    fn auto_replies_skip_machines_lists_and_me() {
        let person = Facts { from_addr: "pat@example.org".into(), ..Default::default() };
        assert!(may_reply(&person, &mine()).is_ok());
        assert!(may_reply(&Facts { auto: Some("auto-replied".into()), ..person.clone() }, &mine()).is_err());
        assert!(may_reply(&Facts { list_id: Some("<x>".into()), ..person.clone() }, &mine()).is_err());
        assert!(may_reply(&Facts { from_addr: "no-reply@shop.com".into(), ..person.clone() }, &mine()).is_err());
        assert!(may_reply(&Facts { from_addr: "MAILER-DAEMON@mx.example.org".into(), ..person.clone() }, &mine()).is_err());
        assert!(may_reply(&Facts { from_addr: "Me@Example.com".into(), ..person.clone() }, &mine()).is_err());
        // Reply-To pointing at a noreply/machine/self address is also skipped.
        assert!(may_reply(&Facts { reply_to: vec![addr("noreply@shop.com")], ..person.clone() }, &mine()).is_err());
        assert!(may_reply(&Facts { reply_to: vec![addr("me@example.com")], ..person.clone() }, &mine()).is_err());
        assert!(may_forward(&person).is_ok());
        assert!(may_forward(&Facts { auto: Some(AUTO_FORWARDED.into()), ..person }).is_err());
    }

    #[test]
    fn auto_reply_threads_and_goes_to_reply_to() {
        let f = Facts {
            from_addr: "pat@example.org".into(),
            reply_to: vec![Addr { name: Some("Doe, John".into()), email: "help@example.org".into() }],
            subject: "Question".into(),
            message_id: Some("<b@x>".into()),
            refs: Some("<a@x>".into()),
            ..Default::default()
        };
        let out = reply_outgoing(&f, 1, None, "Away until Monday.\n<Pat>");
        assert_eq!(out.to, vec!["\"Doe, John\" <help@example.org>"]);
        assert!(out.to[0].parse::<lettre::message::Mailbox>().is_ok());
        assert_eq!(out.subject, "Re: Question");
        assert_eq!(out.in_reply_to.as_deref(), Some("<b@x>"));
        assert_eq!(out.references.as_deref(), Some("<a@x> <b@x>"));
        assert!(out.html.contains("&lt;Pat&gt;"));
        assert_eq!(out.auto_submitted.as_deref(), Some(AUTO_REPLIED));
        assert_eq!(reply_address(&f), "help@example.org");
    }

    #[test]
    fn snooze_times() {
        // Wednesday 2026-11-25 10:00 UTC.
        let now = chrono::Utc.with_ymd_and_hms(2026, 11, 25, 10, 0, 0).unwrap();
        assert_eq!(snooze_until(SnoozeUntil::LaterToday, now), now.timestamp() + 3 * 3600);
        assert_eq!(snooze_until(SnoozeUntil::Tomorrow, now), chrono::Utc.with_ymd_and_hms(2026, 11, 26, 8, 0, 0).unwrap().timestamp());
        assert_eq!(snooze_until(SnoozeUntil::NextWeek, now), chrono::Utc.with_ymd_and_hms(2026, 11, 30, 8, 0, 0).unwrap().timestamp());
    }
}
