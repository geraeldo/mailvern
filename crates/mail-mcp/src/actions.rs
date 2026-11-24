//! Mail CRUD, draft deletion, and sending tools.
//!
//! Every mutating tool resolves the target account's effective permission mode:
//! - In `Approval` ("Ask before acting"): mail changes and draft deletion validate up front and
//!   queue in `agent_actions` for user review; sending is refused.
//! - In `Auto` ("Run automatically"): mail changes run immediately (Trash never permanently
//!   expunges), and sending queues into Mailvern's Undo Send outbox.

use crate::draft::{compose, edit, DraftArgs, EditArgs};
use crate::fmt::{parse_snooze, stamp, subject, to_unix, Names};
use crate::tools::{draft_summary, parse, text, Out};
use crate::Ctx;
use anyhow::{anyhow, bail, Result};
use mail_core::model::{AgentActionPreview, AgentActionSpec, AgentMessagePreview, AgentMode};
use mail_core::text::safe_filename;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};

/// Maximum messages a single mutating call may target.
const MAX_BATCH: usize = 100;

pub fn list(allow_send: bool) -> Vec<Value> {
    let ids_prop = json!({
        "type": "array",
        "items": { "type": "integer" },
        "minItems": 1,
        "maxItems": MAX_BATCH,
        "description": "Message ids (the #number in search_mail results). At most 100 per call.",
    });
    let write_ann = json!({ "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false });
    let dest_ann = json!({ "readOnlyHint": false, "destructiveHint": true, "idempotentHint": false, "openWorldHint": false });

    let mut tools = vec![
        json!({
            "name": "mark_messages",
            "title": "Mark messages read or unread",
            "description": "Mark messages as read (unread: false) or unread (unread: true). Runs right away on automatic accounts, or queues for the user's approval on protected accounts.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "ids": ids_prop,
                    "unread": { "type": "boolean", "description": "true to mark unread, false to mark read." },
                },
                "required": ["ids", "unread"],
                "additionalProperties": false,
            },
            "annotations": write_ann,
        }),
        json!({
            "name": "flag_messages",
            "title": "Flag or unflag messages",
            "description": "Add or remove the flag/star on messages. Runs right away on automatic accounts, or queues for approval on protected accounts.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "ids": ids_prop,
                    "flagged": { "type": "boolean", "description": "true to flag, false to unflag." },
                },
                "required": ["ids", "flagged"],
                "additionalProperties": false,
            },
            "annotations": write_ann,
        }),
        json!({
            "name": "archive_messages",
            "title": "Archive messages",
            "description": "Move messages to their account's Archive folder. Runs right away on automatic accounts, or queues for approval on protected accounts.",
            "inputSchema": {
                "type": "object",
                "properties": { "ids": ids_prop },
                "required": ["ids"],
                "additionalProperties": false,
            },
            "annotations": write_ann,
        }),
        json!({
            "name": "move_messages",
            "title": "Move messages to a folder",
            "description": "Move messages to a specific folder in their account (folderId from list_folders). Runs right away on automatic accounts, or queues for approval on protected accounts.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "ids": ids_prop,
                    "folderId": { "type": "integer", "description": "Destination folder id (from list_folders), in the same account as the messages." },
                },
                "required": ["ids", "folderId"],
                "additionalProperties": false,
            },
            "annotations": write_ann,
        }),
        json!({
            "name": "trash_messages",
            "title": "Move messages to Trash",
            "description": "Move messages to Trash. Never permanently deletes mail: messages already in Trash are refused. Runs right away on automatic accounts, or queues for approval on protected accounts.",
            "inputSchema": {
                "type": "object",
                "properties": { "ids": ids_prop },
                "required": ["ids"],
                "additionalProperties": false,
            },
            "annotations": dest_ann,
        }),
        json!({
            "name": "mark_junk",
            "title": "Mark messages as junk or not junk",
            "description": "Move messages to Junk/Spam (junk: true) or back to Inbox as not junk (junk: false). Runs right away on automatic accounts, or queues for approval on protected accounts.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "ids": ids_prop,
                    "junk": { "type": "boolean", "description": "true to mark as junk, false to move back to Inbox." },
                },
                "required": ["ids", "junk"],
                "additionalProperties": false,
            },
            "annotations": dest_ann,
        }),
        json!({
            "name": "snooze_messages",
            "title": "Snooze messages",
            "description": "Move messages to Snoozed until a later time, when they return to the Inbox as unread. Runs right away on automatic accounts, or queues for approval on protected accounts.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "ids": ids_prop,
                    "until": { "type": "string", "description": "When to bring them back: 'laterToday' (in 3 hours), 'tomorrow' (8:00 tomorrow), 'nextWeek' (Monday 8:00), or a local timestamp 'YYYY-MM-DDTHH:MM'." },
                },
                "required": ["ids", "until"],
                "additionalProperties": false,
            },
            "annotations": write_ann,
        }),
        json!({
            "name": "unsnooze_messages",
            "title": "Unsnooze messages",
            "description": "Bring snoozed messages back to the Inbox now. Runs right away on automatic accounts, or queues for approval on protected accounts.",
            "inputSchema": {
                "type": "object",
                "properties": { "ids": ids_prop },
                "required": ["ids"],
                "additionalProperties": false,
            },
            "annotations": write_ann,
        }),
        json!({
            "name": "delete_draft",
            "title": "Delete a draft",
            "description": "Remove a draft from Drafts by draftId (from create_draft or read_message). Runs right away on automatic accounts, or queues for approval on protected accounts.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "draftId": { "type": "string", "description": "The draft's Message-ID (from create_draft or read_message)." },
                },
                "required": ["draftId"],
                "additionalProperties": false,
            },
            "annotations": dest_ann,
        }),
    ];

    if allow_send {
        tools.push(json!({
            "name": "send_mail",
            "title": "Send an email",
            "description": "Compose and send a new message or reply from an account set to automatic mode (check list_folders). Goes through Mailvern's Undo Send outbox (or sends at sendAt). Accounts in 'ask before acting' mode refuse sending: use create_draft there instead. The account's signature and reply quote are added automatically.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "body": { "type": "string", "description": "The message as plain text. Separate paragraphs with a blank line. Web addresses become links. Don't add a signature: it is added automatically." },
                    "to": { "type": "array", "items": { "type": "string" }, "description": "Recipients, as 'name@example.com' or 'Name <name@example.com>'. Optional for a reply (defaults to the sender)." },
                    "cc": { "type": "array", "items": { "type": "string" } },
                    "bcc": { "type": "array", "items": { "type": "string" } },
                    "subject": { "type": "string", "description": "Optional for a reply (defaults to 'Re: …')." },
                    "attachments": { "type": "array", "items": { "type": "string" }, "description": "Absolute paths of files on this Mac to attach. Hidden files and files in hidden/system folders are refused. 25 MB in all." },
                    "replyTo": { "type": "integer", "description": "Id of the message this replies to." },
                    "replyAll": { "type": "boolean", "description": "With replyTo: also address everyone else on the original message." },
                    "accountId": { "type": "integer", "description": "The account to send from (ids from list_folders). Defaults to the replied-to message's account, else the first account." },
                    "sendAt": { "type": "string", "description": "Optional future local date/time ('YYYY-MM-DDTHH:MM') to schedule sending later." },
                },
                "required": ["body"],
                "additionalProperties": false,
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": true, "idempotentHint": false, "openWorldHint": true },
        }));
        tools.push(json!({
            "name": "send_draft",
            "title": "Send an existing draft",
            "description": "Send a saved draft by draftId (from create_draft or read_message) when its account is set to automatic mode. Removes the draft and queues it in Mailvern's Undo Send outbox (or schedules it at sendAt). Refused on accounts in 'ask before acting' mode.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "draftId": { "type": "string", "description": "The draft's Message-ID (from create_draft or read_message)." },
                    "sendAt": { "type": "string", "description": "Optional future local date/time ('YYYY-MM-DDTHH:MM') to schedule sending later." },
                },
                "required": ["draftId"],
                "additionalProperties": false,
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": true, "idempotentHint": false, "openWorldHint": true },
        }));
    }

    tools
}

pub async fn call(ctx: &Ctx, name: &str, args: Value) -> Option<Result<Out>> {
    Some(match name {
        "mark_messages" => mark_messages(ctx, args),
        "flag_messages" => flag_messages(ctx, args),
        "archive_messages" => archive_messages(ctx, args),
        "move_messages" => move_messages(ctx, args),
        "trash_messages" => trash_messages(ctx, args),
        "mark_junk" => mark_junk(ctx, args),
        "snooze_messages" => snooze_messages(ctx, args),
        "unsnooze_messages" => unsnooze_messages(ctx, args),
        "delete_draft" => delete_draft(ctx, args),
        "send_mail" => send_mail(ctx, args).await,
        "send_draft" => send_draft(ctx, args).await,
        _ => return None,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct IdsArg {
    ids: Vec<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MarkArgs {
    ids: Vec<i64>,
    unread: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FlagArgs {
    ids: Vec<i64>,
    flagged: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MoveArgs {
    ids: Vec<i64>,
    folder_id: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct JunkArgs {
    ids: Vec<i64>,
    junk: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SnoozeArgs {
    ids: Vec<i64>,
    until: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeleteDraftArgs {
    draft_id: String,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
struct SendMailArgs {
    account_id: Option<i64>,
    to: Option<Vec<String>>,
    cc: Option<Vec<String>>,
    bcc: Vec<String>,
    subject: Option<String>,
    body: String,
    attachments: Vec<String>,
    reply_to: Option<i64>,
    reply_all: bool,
    send_at: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SendDraftArgs {
    draft_id: String,
    #[serde(default)]
    send_at: Option<String>,
}

/// Validates, deduplicates, and partitions message ids by account id (preserving order).
fn validate_and_group(ctx: &Ctx, raw_ids: &[i64], refuse_trash: bool) -> Result<BTreeMap<i64, Vec<i64>>> {
    if raw_ids.is_empty() {
        bail!("Provide at least one message id.");
    }
    let mut seen = HashSet::new();
    let mut ids = Vec::new();
    for &id in raw_ids {
        if seen.insert(id) {
            ids.push(id);
        }
    }
    if ids.len() > MAX_BATCH {
        bail!("At most {MAX_BATCH} messages can be changed in a single call.");
    }
    let mut by_account: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    for id in ids {
        let (acc_id, _, role) = ctx.engine.message_folder_role(id).map_err(|_| anyhow!("No message #{id}."))?;
        if refuse_trash && role.as_deref() == Some("trash") {
            bail!("Message #{id} is already in Trash. Agents can't permanently delete mail; empty Trash in Mailvern.");
        }
        by_account.entry(acc_id).or_default().push(id);
    }
    Ok(by_account)
}

fn message_previews(ctx: &Ctx, ids: &[i64]) -> Vec<AgentMessagePreview> {
    let rows = ctx.engine.summaries(ids).unwrap_or_default();
    ids.iter()
        .filter_map(|id| rows.iter().find(|r| r.id == *id))
        .map(|m| AgentMessagePreview {
            id: m.id,
            subject: m.subject.clone(),
            from_name: m.from_name.clone(),
            from_addr: m.from_addr.clone(),
        })
        .collect()
}

fn plural_msg(n: usize) -> String {
    if n == 1 { "1 message".into() } else { format!("{n} messages") }
}

fn account_tag(names: &Names, account_id: i64) -> String {
    if names.accounts.len() <= 1 {
        return String::new();
    }
    names
        .account(account_id)
        .map(|a| format!(" in {}", a.email))
        .unwrap_or_default()
}

struct BatchLabel<'a> {
    imperative: &'a str,
    past: &'a str,
    target_folder: Option<String>,
    until: Option<i64>,
}

/// Runs or queues a per-account batch of messages according to each account's effective mode.
fn run_mail_batches<F, E>(
    ctx: &Ctx,
    by_account: BTreeMap<i64, Vec<i64>>,
    label: BatchLabel<'_>,
    make_spec: F,
    run_auto: E,
) -> Result<Out>
where
    F: Fn(Vec<i64>) -> AgentActionSpec,
    E: Fn(&[i64]) -> Result<()>,
{
    let names = Names::load(ctx)?;
    let mut lines = Vec::new();
    let mut any_queued = false;

    for (acc_id, ids) in by_account {
        let count_str = plural_msg(ids.len());
        let suffix = match (&label.target_folder, label.until) {
            (Some(f), _) => format!(" to {f}"),
            (None, Some(t)) => format!(" until {}", stamp(t)),
            _ => String::new(),
        };
        let summary = format!("{} {count_str}{suffix}", label.imperative);
        let preview = AgentActionPreview {
            messages: message_previews(ctx, &ids),
            target_folder: label.target_folder.clone(),
            until: label.until,
            detail: None,
        };
        let spec = make_spec(ids.clone());
        let acc_str = account_tag(&names, acc_id);

        if names.perms.for_account(acc_id) == AgentMode::Auto {
            run_auto(&ids)?;
            let _ = ctx.engine.record_auto_agent_action(Some(acc_id), spec, summary, preview);
            lines.push(format!("{} {count_str}{suffix}{acc_str}.", label.past));
        } else {
            let queued = ctx.engine.queue_agent_action(Some(acc_id), spec, summary.clone(), preview)?;
            any_queued = true;
            lines.push(format!("Queued for approval (#{}): {summary}{acc_str}.", queued.id));
        }
    }

    if any_queued {
        lines.push("Tell the user to review and approve pending actions in Mailvern › Settings › Agent access.".into());
    }
    Ok(text(lines.join("\n")))
}

fn mark_messages(ctx: &Ctx, args: Value) -> Result<Out> {
    let a: MarkArgs = parse(args)?;
    let seen = !a.unread;
    let by_account = validate_and_group(ctx, &a.ids, false)?;
    let (imperative, past) = if seen { ("Mark as read:", "Marked as read:") } else { ("Mark as unread:", "Marked as unread:") };
    run_mail_batches(
        ctx,
        by_account,
        BatchLabel { imperative, past, target_folder: None, until: None },
        |ids| AgentActionSpec::MarkSeen { ids, seen },
        |ids| ctx.engine.set_seen(ids, seen),
    )
}

fn flag_messages(ctx: &Ctx, args: Value) -> Result<Out> {
    let a: FlagArgs = parse(args)?;
    let by_account = validate_and_group(ctx, &a.ids, false)?;
    let (imperative, past) = if a.flagged { ("Flag", "Flagged") } else { ("Unflag", "Unflagged") };
    run_mail_batches(
        ctx,
        by_account,
        BatchLabel { imperative, past, target_folder: None, until: None },
        |ids| AgentActionSpec::SetFlagged { ids, flagged: a.flagged },
        |ids| ctx.engine.set_flagged(ids, a.flagged),
    )
}

fn archive_messages(ctx: &Ctx, args: Value) -> Result<Out> {
    let a: IdsArg = parse(args)?;
    let by_account = validate_and_group(ctx, &a.ids, false)?;
    run_mail_batches(
        ctx,
        by_account,
        BatchLabel { imperative: "Archive", past: "Archived", target_folder: None, until: None },
        |ids| AgentActionSpec::Archive { ids },
        |ids| ctx.engine.archive(ids),
    )
}

fn move_messages(ctx: &Ctx, args: Value) -> Result<Out> {
    let a: MoveArgs = parse(args)?;
    let names = Names::load(ctx)?;
    let folder = names
        .folders
        .iter()
        .find(|f| f.id == a.folder_id)
        .ok_or_else(|| anyhow!("No folder {}. Use list_folders to see folder ids.", a.folder_id))?;
    let by_account = validate_and_group(ctx, &a.ids, false)?;
    if by_account.keys().any(|&acc| acc != folder.account_id) {
        bail!("Messages can only be moved to a folder in their own account.");
    }
    let folder_label = folder.name.clone();
    run_mail_batches(
        ctx,
        by_account,
        BatchLabel { imperative: "Move", past: "Moved", target_folder: Some(folder_label), until: None },
        |ids| AgentActionSpec::MoveTo { ids, folder_id: a.folder_id },
        |ids| ctx.engine.move_to(ids, a.folder_id),
    )
}

fn trash_messages(ctx: &Ctx, args: Value) -> Result<Out> {
    let a: IdsArg = parse(args)?;
    let by_account = validate_and_group(ctx, &a.ids, true)?;
    run_mail_batches(
        ctx,
        by_account,
        BatchLabel { imperative: "Move to Trash:", past: "Moved to Trash:", target_folder: None, until: None },
        |ids| AgentActionSpec::Trash { ids },
        |ids| ctx.engine.move_to_trash_only(ids).map(|_| ()),
    )
}

fn mark_junk(ctx: &Ctx, args: Value) -> Result<Out> {
    let a: JunkArgs = parse(args)?;
    let by_account = validate_and_group(ctx, &a.ids, false)?;
    let (imperative, past) = if a.junk { ("Mark as junk:", "Marked as junk:") } else { ("Mark as not junk:", "Moved back to Inbox:") };
    run_mail_batches(
        ctx,
        by_account,
        BatchLabel { imperative, past, target_folder: None, until: None },
        |ids| AgentActionSpec::Junk { ids, junk: a.junk },
        |ids| if a.junk { ctx.engine.junk(ids) } else { ctx.engine.not_junk(ids) },
    )
}

fn snooze_messages(ctx: &Ctx, args: Value) -> Result<Out> {
    let a: SnoozeArgs = parse(args)?;
    let until = parse_snooze(&a.until)?;
    if until <= chrono::Utc::now().timestamp() {
        bail!("until must be in the future.");
    }
    let by_account = validate_and_group(ctx, &a.ids, false)?;
    run_mail_batches(
        ctx,
        by_account,
        BatchLabel { imperative: "Snooze", past: "Snoozed", target_folder: None, until: Some(until) },
        |ids| AgentActionSpec::Snooze { ids, until },
        |ids| ctx.engine.snooze(ids, until),
    )
}

fn unsnooze_messages(ctx: &Ctx, args: Value) -> Result<Out> {
    let a: IdsArg = parse(args)?;
    let by_account = validate_and_group(ctx, &a.ids, false)?;
    run_mail_batches(
        ctx,
        by_account,
        BatchLabel { imperative: "Unsnooze", past: "Unsnoozed", target_folder: None, until: None },
        |ids| AgentActionSpec::Unsnooze { ids },
        |ids| ctx.engine.unsnooze(ids),
    )
}

fn delete_draft(ctx: &Ctx, args: Value) -> Result<Out> {
    let a: DeleteDraftArgs = parse(args)?;
    let trimmed = a.draft_id.trim();
    if trimmed.is_empty() {
        bail!("Invalid arguments: draftId is required.");
    }
    let (msg_id, acc_id) = ctx.engine.find_draft(trimmed)?.ok_or_else(|| {
        anyhow!("No draft {trimmed} in any Drafts folder. Find drafts with search_mail (folder \"drafts\").")
    })?;
    let canonical_id = if trimmed.starts_with('<') { trimmed.to_string() } else { format!("<{trimmed}>") };
    let names = Names::load(ctx)?;
    let previews = message_previews(ctx, &[msg_id]);
    let subj = previews.first().map(|p| subject(&p.subject)).unwrap_or("(no subject)").to_string();
    let summary = format!("Delete draft “{subj}”");
    let preview = AgentActionPreview {
        messages: previews,
        target_folder: None,
        until: None,
        detail: Some(format!("draftId {canonical_id}")),
    };
    let spec = AgentActionSpec::DeleteDraft { account_id: acc_id, message_id: canonical_id.clone() };
    let acc_str = account_tag(&names, acc_id);

    if names.perms.for_account(acc_id) == AgentMode::Auto {
        ctx.engine.delete_draft(acc_id, &canonical_id)?;
        let _ = ctx.engine.record_auto_agent_action(Some(acc_id), spec, summary, preview);
        Ok(text(format!("Deleted draft “{subj}”{acc_str}.")))
    } else {
        let queued = ctx.engine.queue_agent_action(Some(acc_id), spec, summary.clone(), preview)?;
        Ok(text(format!(
            "Queued for approval (#{}): {summary}{acc_str}.\nTell the user to review and approve it in Mailvern › Settings › Agent access.",
            queued.id
        )))
    }
}

fn parse_send_at(raw: &Option<String>) -> Result<Option<i64>> {
    let Some(s) = raw.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let unix = to_unix(s, "sendAt")?;
    if unix <= chrono::Utc::now().timestamp() {
        bail!("sendAt must be in the future.");
    }
    Ok(Some(unix))
}

fn send_confirmation(ctx: &Ctx, account_email: &str, outbox_id: i64, send_at: Option<i64>, summary: &str) -> String {
    let timing = match send_at {
        Some(t) => format!("Scheduled to send from {account_email} at {} (outbox #{outbox_id}).", stamp(t)),
        None => {
            let undo = ctx
                .engine
                .setting("undo_send")
                .ok()
                .flatten()
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(10)
                .clamp(0, 120);
            if undo > 0 {
                format!("Queued to send from {account_email} in {undo} seconds (Undo Send window, outbox #{outbox_id}).")
            } else {
                format!("Queued to send from {account_email} now (outbox #{outbox_id}).")
            }
        }
    };
    format!("{timing}\n\n{summary}")
}

async fn send_mail(ctx: &Ctx, args: Value) -> Result<Out> {
    let a: SendMailArgs = parse(args)?;
    let send_at = parse_send_at(&a.send_at)?;
    if a.body.trim().is_empty() && a.attachments.is_empty() {
        bail!("The message has no body. Write the message in body.");
    }
    let source = match a.reply_to {
        Some(id) => Some(ctx.engine.message(id).await?),
        None => None,
    };
    let accounts = ctx.engine.accounts()?;
    let account_id = a.account_id.or(source.as_ref().map(|s| s.summary.account_id)).or(accounts.first().map(|x| x.id));
    let account = account_id
        .and_then(|id| accounts.iter().find(|x| x.id == id))
        .ok_or_else(|| anyhow!("No such account. Use list_folders to see the accounts."))?;

    let perms = ctx.engine.agent_permissions()?;
    if perms.for_account(account.id) != AgentMode::Auto {
        bail!(
            "Sending mail is disabled for {} (Ask before acting mode). Save a draft with create_draft instead so the user can review and send it in Mailvern.",
            account.email
        );
    }

    let draft_args = DraftArgs {
        account_id: a.account_id,
        to: a.to,
        cc: a.cc,
        bcc: a.bcc,
        subject: a.subject,
        body: a.body,
        attachments: a.attachments,
        reply_to: a.reply_to,
        reply_all: a.reply_all,
    };
    let mine: Vec<String> = accounts.iter().map(|x| x.email.clone()).collect();
    let out = compose(&draft_args, account, &mine, source.as_ref())?;
    let summary = draft_summary(account, &out);
    let subj = out.subject.clone();
    let recipients = out.to.clone();
    let outbox_id = ctx.engine.send(out, send_at)?;
    ctx.engine.notify_agent_sent(outbox_id, account.id, subj.clone(), recipients.clone());
    let _ = ctx.engine.record_auto_agent_action(
        Some(account.id),
        AgentActionSpec::SendMail { to: recipients.clone(), subject: subj.clone(), send_at },
        format!("Send “{}” to {}", subject(&subj), if recipients.is_empty() { "(recipients)".into() } else { recipients.join(", ") }),
        AgentActionPreview {
            messages: vec![],
            target_folder: None,
            until: send_at,
            detail: Some(summary.clone()),
        },
    );
    Ok(text(send_confirmation(ctx, &account.email, outbox_id, send_at, &summary)))
}

async fn send_draft(ctx: &Ctx, args: Value) -> Result<Out> {
    let a: SendDraftArgs = parse(args)?;
    let send_at = parse_send_at(&a.send_at)?;
    if a.draft_id.trim().is_empty() {
        bail!("Invalid arguments: draftId is required.");
    }
    let (message_id, account_id) = ctx.engine.find_draft(&a.draft_id)?.ok_or_else(|| {
        anyhow!("No draft {} in any Drafts folder. Find drafts with search_mail (folder \"drafts\").", a.draft_id.trim())
    })?;
    let accounts = ctx.engine.accounts()?;
    let account = accounts.iter().find(|x| x.id == account_id).ok_or_else(|| anyhow!("The draft's account is gone."))?;

    let perms = ctx.engine.agent_permissions()?;
    if perms.for_account(account.id) != AgentMode::Auto {
        bail!(
            "Sending mail is disabled for {} (Ask before acting mode). The draft is saved in Drafts for the user to review and send in Mailvern.",
            account.email
        );
    }

    let existing = ctx.engine.message(message_id).await?;
    struct TempCleanup(Option<std::path::PathBuf>);
    impl Drop for TempCleanup {
        fn drop(&mut self) {
            if let Some(dir) = &self.0 {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
    }
    let mut kept = Vec::new();
    let mut _cleanup = TempCleanup(None);
    if !existing.attachments.is_empty() {
        let dir = std::env::temp_dir().join("Mailvern").join("agents").join(format!("send-draft-{}", uuid::Uuid::new_v4().simple()));
        _cleanup.0 = Some(dir.clone());
        for (i, (name, bytes)) in ctx.engine.all_attachments(message_id).await?.into_iter().enumerate() {
            let path = dir.join(i.to_string()).join(safe_filename(&name));
            std::fs::create_dir_all(path.parent().unwrap())?;
            std::fs::write(&path, &bytes)?;
            kept.push(path.to_string_lossy().into_owned());
        }
    }

    let out = edit(&EditArgs { draft_id: a.draft_id, ..Default::default() }, account, &existing, kept)?;
    let summary = draft_summary(account, &out);
    let subj = out.subject.clone();
    let recipients = out.to.clone();
    let outbox_id = ctx.engine.send(out, send_at)?;
    ctx.engine.notify_agent_sent(outbox_id, account.id, subj.clone(), recipients.clone());
    let _ = ctx.engine.record_auto_agent_action(
        Some(account.id),
        AgentActionSpec::SendMail { to: recipients.clone(), subject: subj.clone(), send_at },
        format!("Send draft “{}” to {}", subject(&subj), if recipients.is_empty() { "(recipients)".into() } else { recipients.join(", ") }),
        AgentActionPreview {
            messages: vec![],
            target_folder: None,
            until: send_at,
            detail: Some(summary.clone()),
        },
    );
    Ok(text(send_confirmation(ctx, &account.email, outbox_id, send_at, &summary)))
}
