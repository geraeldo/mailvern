//! RFC 5545 / iMIP calendar invite (.ics) parser and RSVP reply generator.

use crate::model::{Addr, CalendarAttendee, CalendarInvite};
use chrono::{NaiveDate, NaiveDateTime, TimeZone, Utc};
use mail_parser::{Message, MimeHeaders, PartType};

/// Extracts the raw `.ics` payload from a parsed email (inline `text/calendar` or `.ics` attachment).
pub fn extract_raw_ics(m: &Message<'_>) -> Option<Vec<u8>> {
    // Prefer an explicit text/calendar part anywhere in the MIME tree.
    for part in &m.parts {
        if let Some(ct) = part.content_type() {
            if ct.c_type.eq_ignore_ascii_case("text") && ct.subtype().is_some_and(|s| s.eq_ignore_ascii_case("calendar")) {
                let bytes = match &part.body {
                    PartType::Text(t) => t.as_bytes().to_vec(),
                    PartType::Binary(b) => b.to_vec(),
                    _ => part.contents().to_vec(),
                };
                if !bytes.is_empty() {
                    return Some(bytes);
                }
            }
        }
        if let Some(name) = part.attachment_name() {
            if name.to_ascii_lowercase().ends_with(".ics") {
                let bytes = part.contents().to_vec();
                if !bytes.is_empty() {
                    return Some(bytes);
                }
            }
        }
    }
    None
}

/// Unfolds RFC 5545 folded lines (CRLF/LF followed by single space or tab).
fn unfold_ics(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in text.replace("\r\n", "\n").replace('\r', "\n").split('\n') {
        if (raw.starts_with(' ') || raw.starts_with('\t')) && !lines.is_empty() {
            if let Some(last) = lines.last_mut() {
                last.push_str(&raw[1..]);
            }
        } else {
            lines.push(raw.to_string());
        }
    }
    lines
}

fn unescape_ics(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') | Some('N') => out.push('\n'),
                Some(',') => out.push(','),
                Some(';') => out.push(';'),
                Some('\\') => out.push('\\'),
                Some(other) => out.push(other),
                None => break,
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Splits an ICS property line into `(name, params, value)`.
fn parse_prop(line: &str) -> Option<(String, Vec<(String, String)>, String)> {
    // Find first ':' outside quotes.
    let mut in_quotes = false;
    let mut colon = None;
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_quotes = !in_quotes,
            ':' if !in_quotes => {
                colon = Some(i);
                break;
            }
            _ => {}
        }
    }
    let colon = colon?;
    let lhs = &line[..colon];
    let val = line[colon + 1..].to_string();

    // Split lhs on ';' outside quotes.
    let mut parts = Vec::new();
    let mut start = 0;
    in_quotes = false;
    for (i, c) in lhs.char_indices() {
        match c {
            '"' => in_quotes = !in_quotes,
            ';' if !in_quotes => {
                parts.push(&lhs[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&lhs[start..]);

    let name = parts.first()?.trim().to_ascii_uppercase();
    let mut params = Vec::new();
    for p in parts.iter().skip(1) {
        if let Some((k, v)) = p.split_once('=') {
            params.push((k.trim().to_ascii_uppercase(), v.trim().trim_matches('"').to_string()));
        }
    }
    Some((name, params, val))
}

fn param<'a>(params: &'a [(String, String)], key: &str) -> Option<&'a str> {
    params.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str())
}

fn strip_mailto(v: &str) -> String {
    let t = v.trim();
    if t.len() >= 7 && t[..7].eq_ignore_ascii_case("mailto:") {
        t[7..].trim().to_string()
    } else {
        t.to_string()
    }
}

/// Parses an ICS DTSTART/DTEND value into `(unix_timestamp_opt, all_day)`.
fn parse_ics_time(val: &str, params: &[(String, String)]) -> (Option<i64>, bool) {
    let v = val.trim();
    let is_date = param(params, "VALUE").is_some_and(|x| x.eq_ignore_ascii_case("DATE")) || (v.len() == 8 && v.chars().all(|c| c.is_ascii_digit()));
    if is_date {
        let ts = NaiveDate::parse_from_str(v, "%Y%m%d")
            .ok()
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|dt| Utc.from_utc_datetime(&dt).timestamp());
        return (ts, true);
    }
    if let Some(utc) = v.strip_suffix('Z').or_else(|| v.strip_suffix('z')) {
        let ts = NaiveDateTime::parse_from_str(utc, "%Y%m%dT%H%M%S")
            .ok()
            .map(|dt| Utc.from_utc_datetime(&dt).timestamp());
        return (ts, false);
    }
    // Floating / local / TZID-qualified datetime: interpret in local timezone as a clean display approximation.
    let ts = NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M%S")
        .ok()
        .and_then(|dt| chrono::Local.from_local_datetime(&dt).earliest())
        .map(|dt| dt.timestamp());
    (ts, false)
}

/// Parses an `.ics` payload into a `CalendarInvite` (first VEVENT).
pub fn parse_ics(bytes: &[u8]) -> Option<CalendarInvite> {
    let text = String::from_utf8_lossy(bytes);
    let lines = unfold_ics(&text);
    let mut method = "REQUEST".to_string();
    let mut in_event = false;
    let mut found_event = false;
    let mut uid = String::new();
    let mut summary = String::new();
    let mut location: Option<String> = None;
    let mut description: Option<String> = None;
    let mut start: Option<i64> = None;
    let mut end: Option<i64> = None;
    let mut dtstart_raw = String::new();
    let mut dtend_raw: Option<String> = None;
    let mut all_day = false;
    let mut tzid: Option<String> = None;
    let mut organizer: Option<Addr> = None;
    let mut attendees: Vec<CalendarAttendee> = Vec::new();
    let mut sequence: i64 = 0;

    for line in lines {
        let t = line.trim();
        if t.eq_ignore_ascii_case("BEGIN:VEVENT") {
            in_event = true;
            found_event = true;
            continue;
        }
        if t.eq_ignore_ascii_case("END:VEVENT") {
            if in_event {
                break;
            }
            continue;
        }
        let Some((name, params, val)) = parse_prop(t) else { continue };
        if !in_event {
            if name == "METHOD" && !val.trim().is_empty() {
                method = val.trim().to_ascii_uppercase();
            }
            continue;
        }
        match name.as_str() {
            "UID" => uid = val.trim().to_string(),
            "SUMMARY" => summary = unescape_ics(val.trim()),
            "LOCATION" => {
                let s = unescape_ics(val.trim());
                if !s.is_empty() {
                    location = Some(s);
                }
            }
            "DESCRIPTION" => {
                let s = unescape_ics(val.trim());
                if !s.is_empty() {
                    description = Some(s.chars().take(600).collect());
                }
            }
            "DTSTART" => {
                dtstart_raw = val.trim().to_string();
                tzid = param(&params, "TZID").map(str::to_string);
                let (ts, ad) = parse_ics_time(&val, &params);
                start = ts;
                all_day = ad;
            }
            "DTEND" => {
                dtend_raw = Some(val.trim().to_string());
                let (ts, _) = parse_ics_time(&val, &params);
                end = ts;
            }
            "ORGANIZER" => {
                let email = strip_mailto(&val);
                if !email.is_empty() {
                    let cn = param(&params, "CN").map(unescape_ics).filter(|s| !s.is_empty());
                    organizer = Some(Addr { name: cn, email });
                }
            }
            "ATTENDEE" => {
                let email = strip_mailto(&val);
                if !email.is_empty() {
                    let cn = param(&params, "CN").map(unescape_ics).filter(|s| !s.is_empty());
                    let status = param(&params, "PARTSTAT")
                        .map(|s| s.to_ascii_uppercase())
                        .unwrap_or_else(|| "NEEDS-ACTION".into());
                    attendees.push(CalendarAttendee { name: cn, email, status });
                }
            }
            "SEQUENCE" => {
                if let Ok(n) = val.trim().parse::<i64>() {
                    sequence = n;
                }
            }
            _ => {}
        }
    }

    if !found_event || (uid.is_empty() && summary.is_empty()) {
        return None;
    }

    Some(CalendarInvite {
        uid,
        method,
        summary: if summary.is_empty() { "(No title)".into() } else { summary },
        location,
        description,
        start,
        end,
        all_day,
        tzid,
        dtstart_raw,
        dtend_raw,
        organizer,
        attendees,
        sequence,
        my_status: None,
    })
}

/// Parses the first calendar invite in a raw RFC 822 message, if any.
pub fn parse_from_message(m: &Message<'_>) -> Option<CalendarInvite> {
    let raw = extract_raw_ics(m)?;
    parse_ics(&raw)
}

fn escape_ics_text(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace(',', "\\,")
        .replace(';', "\\;")
}

/// Builds an RFC 5545 `METHOD:REPLY` VCALENDAR payload for an RSVP (`ACCEPTED`, `TENTATIVE`, or `DECLINED`).
pub fn build_rsvp_ics(invite: &CalendarInvite, responder_name: &str, responder_email: &str, status: &str) -> String {
    let dtstamp = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let mut lines = vec![
        "BEGIN:VCALENDAR".to_string(),
        "PRODID:-//Mailvern//Calendar RSVP//EN".to_string(),
        "VERSION:2.0".to_string(),
        "METHOD:REPLY".to_string(),
        "BEGIN:VEVENT".to_string(),
        format!("UID:{}", invite.uid),
        format!("DTSTAMP:{dtstamp}"),
        format!("SEQUENCE:{}", invite.sequence),
        format!("SUMMARY:{}", escape_ics_text(&invite.summary)),
    ];
    if !invite.dtstart_raw.is_empty() {
        if invite.all_day {
            lines.push(format!("DTSTART;VALUE=DATE:{}", invite.dtstart_raw));
        } else if let Some(tz) = &invite.tzid {
            lines.push(format!("DTSTART;TZID={tz}:{}", invite.dtstart_raw));
        } else {
            lines.push(format!("DTSTART:{}", invite.dtstart_raw));
        }
    }
    if let Some(end_raw) = &invite.dtend_raw {
        if invite.all_day {
            lines.push(format!("DTEND;VALUE=DATE:{end_raw}"));
        } else if let Some(tz) = &invite.tzid {
            lines.push(format!("DTEND;TZID={tz}:{end_raw}"));
        } else {
            lines.push(format!("DTEND:{end_raw}"));
        }
    }
    if let Some(org) = &invite.organizer {
        if let Some(cn) = &org.name {
            lines.push(format!("ORGANIZER;CN=\"{}\":mailto:{}", cn.replace('"', ""), org.email));
        } else {
            lines.push(format!("ORGANIZER:mailto:{}", org.email));
        }
    }
    let clean_name = responder_name.trim().replace('"', "");
    if clean_name.is_empty() {
        lines.push(format!("ATTENDEE;PARTSTAT={status}:mailto:{responder_email}"));
    } else {
        lines.push(format!("ATTENDEE;CN=\"{clean_name}\";PARTSTAT={status}:mailto:{responder_email}"));
    }
    lines.push("END:VEVENT".to_string());
    lines.push("END:VCALENDAR".to_string());
    lines.join("\r\n") + "\r\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_folded_invite_and_generates_rsvp() {
        let ics = concat!(
            "BEGIN:VCALENDAR\r\n",
            "VERSION:2.0\r\n",
            "METHOD:REQUEST\r\n",
            "BEGIN:VEVENT\r\n",
            "UID:evt-123@example.com\r\n",
            "DTSTART:20261201T150000Z\r\n",
            "DTEND:20261201T160000Z\r\n",
            "SUMMARY:Q4 Planning\r\n",
            "  Review\r\n",
            "LOCATION:Room 4B\\, Floor 2\r\n",
            "ORGANIZER;CN=\"Maya Chen\":mailto:maya@acme.co\r\n",
            "ATTENDEE;CN=\"Alex\":mailto:alex@acme.co\r\n",
            "SEQUENCE:2\r\n",
            "END:VEVENT\r\n",
            "END:VCALENDAR\r\n"
        );
        let inv = parse_ics(ics.as_bytes()).unwrap();
        assert_eq!(inv.uid, "evt-123@example.com");
        assert_eq!(inv.method, "REQUEST");
        assert_eq!(inv.summary, "Q4 Planning Review");
        assert_eq!(inv.location.as_deref(), Some("Room 4B, Floor 2"));
        assert_eq!(inv.organizer.as_ref().unwrap().email, "maya@acme.co");
        assert_eq!(inv.attendees.len(), 1);
        assert!(!inv.all_day);
        assert!(inv.start.is_some());

        let reply = build_rsvp_ics(&inv, "Alex", "alex@acme.co", "ACCEPTED");
        assert!(reply.contains("METHOD:REPLY"));
        assert!(reply.contains("UID:evt-123@example.com"));
        assert!(reply.contains("PARTSTAT=ACCEPTED:mailto:alex@acme.co"));
    }
}
