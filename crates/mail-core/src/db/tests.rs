use super::*;

fn temp_db() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("mailvern-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("mailvern.db")
}

fn version(path: &Path) -> i64 {
    Connection::open(path).unwrap().query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap()
}

fn account() -> Account {
    Account {
        id: 0,
        name: "Me".into(),
        email: "me@example.com".into(),
        imap_host: "imap.example.com".into(),
        imap_port: 993,
        smtp_host: "smtp.example.com".into(),
        smtp_port: 465,
        color: String::new(),
        signature: "<b>Me</b>".into(),
        position: 0,
        label: "Work".into(),
        ..Default::default()
    }
}

fn msg(uid: u32, seen: bool) -> NewMessage {
    NewMessage {
        uid,
        message_id: None,
        in_reply_to: None,
        refs: None,
        subject: format!("m{uid}"),
        from_name: String::new(),
        from_addr: "a@example.com".into(),
        to: vec![],
        cc: vec![],
        reply_to: vec![],
        date: uid as i64,
        seen,
        flagged: false,
        has_attachments: false,
        size: 0,
        list_id: None,
        auto: None,
    }
}

fn mail(uid: u32, mid: &str, irt: Option<&str>, refs: Option<&str>, date: i64) -> NewMessage {
    NewMessage {
        message_id: Some(mid.into()),
        in_reply_to: irt.map(String::from),
        refs: refs.map(String::from),
        date,
        ..msg(uid, false)
    }
}

fn thread_of(db: &Db, folder: i64, uid: u32) -> String {
    db.c().query_row("SELECT thread_id FROM messages WHERE folder_id = ? AND uid = ?", params![folder, uid], |r| r.get(0)).unwrap()
}

#[test]
fn replies_join_their_conversation_in_any_arrival_order() {
    let db = Db::open(&temp_db()).unwrap();
    let acc = db.save_account(&account()).unwrap();
    let inbox = db.upsert_folder(acc, "INBOX", "Inbox", Some("inbox")).unwrap();
    let sent = db.upsert_folder(acc, "Sent", "Sent", Some("sent")).unwrap();

    // A reply that only has In-Reply-To arrives before its parent.
    db.insert_messages(acc, inbox, &[mail(3, "<c>", Some("<b>"), None, 30)]).unwrap();
    // The parent refers to the root, which we never see.
    db.insert_messages(acc, sent, &[mail(1, "<b>", Some("<a>"), Some("<a>"), 20)]).unwrap();
    // An unrelated message.
    db.insert_messages(acc, inbox, &[mail(4, "<x>", None, None, 40)]).unwrap();
    // A later reply quoting the full chain.
    db.insert_messages(acc, inbox, &[mail(5, "<d>", Some("<c>"), Some("<a> <b> <c>"), 50)]).unwrap();

    let t = thread_of(&db, inbox, 3);
    assert_eq!(t, "<a>");
    assert_eq!(thread_of(&db, sent, 1), t);
    assert_eq!(thread_of(&db, inbox, 5), t);
    assert_eq!(thread_of(&db, inbox, 4), "<x>");

    // The inbox lists one row per conversation, standing for its inbox messages.
    let rows = db.list_threads(&View::Folder { folder_id: inbox }, 0, 50).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].subject, "m5");
    assert_eq!(rows[0].ids.len(), 2);
    assert_eq!(rows[0].count, 3, "count includes the Sent reply");
    assert!(!rows[0].seen);
    assert_eq!(rows[1].count, 1);

    // The conversation spans folders, oldest first.
    let thread = db.thread(rows[0].id).unwrap();
    assert_eq!(thread.iter().map(|m| m.subject.as_str()).collect::<Vec<_>>(), vec!["m1", "m3", "m5"]);
}

#[test]
fn existing_mail_gets_conversations_on_upgrade() {
    let path = temp_db();
    {
        let db = Db::open(&path).unwrap();
        let acc = db.save_account(&account()).unwrap();
        let inbox = db.upsert_folder(acc, "INBOX", "Inbox", Some("inbox")).unwrap();
        db.insert_messages(acc, inbox, &[mail(1, "<a>", None, None, 1), mail(2, "<b>", Some("<a>"), None, 2)]).unwrap();
        db.c().execute("UPDATE messages SET thread_id = NULL", []).unwrap();
    }
    let db = Db::open(&path).unwrap();
    let n: i64 = db.c().query_row("SELECT COUNT(DISTINCT thread_id) FROM messages WHERE thread_id IS NOT NULL", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 1);
}

#[test]
fn search_hits_stay_out_of_the_sync_window() {
    let db = Db::open(&temp_db()).unwrap();
    let acc = db.save_account(&account()).unwrap();
    let inbox = db.upsert_folder(acc, "INBOX", "Inbox", Some("inbox")).unwrap();
    db.insert_messages(acc, inbox, &[msg(500, true), msg(501, true)]).unwrap();
    db.insert_messages_as(acc, inbox, &[msg(7, true)], true).unwrap();

    assert_eq!(db.uid_range(inbox).unwrap(), (500, 501));
    assert_eq!(db.sparse_uids_below(inbox, 500).unwrap(), vec![7]);
    assert_eq!(db.missing_uids(inbox, &[7, 8, 500]).unwrap(), vec![8]);

    // The server no longer has it.
    assert!(db.reconcile_uids(inbox, &[7], &[], &Default::default()).unwrap());
    assert!(db.sparse_uids_below(inbox, 500).unwrap().is_empty());

    // Sync scrolling down to a hit takes it into the window.
    db.insert_messages_as(acc, inbox, &[msg(499, true)], true).unwrap();
    db.insert_messages(acc, inbox, &[msg(499, true)]).unwrap();
    assert_eq!(db.uid_range(inbox).unwrap(), (499, 501));
}

#[test]
fn search_stays_in_its_scope() {
    let db = Db::open(&temp_db()).unwrap();
    let a = db.save_account(&account()).unwrap();
    let b = db.save_account(&Account { email: "other@example.com".into(), ..account() }).unwrap();
    let a_inbox = db.upsert_folder(a, "INBOX", "Inbox", Some("inbox")).unwrap();
    let a_work = db.upsert_folder(a, "Work", "Work", None).unwrap();
    let b_inbox = db.upsert_folder(b, "INBOX", "Inbox", Some("inbox")).unwrap();
    let invoice = |uid| NewMessage { subject: "Invoice due".into(), ..msg(uid, true) };
    db.insert_messages(a, a_inbox, &[invoice(1)]).unwrap();
    db.insert_messages(a, a_work, &[invoice(2)]).unwrap();
    db.insert_messages(b, b_inbox, &[invoice(3), msg(4, true)]).unwrap();

    let hits = |scope: SearchScope| {
        let mut f: Vec<i64> = db.search("invoice", &scope, 50).unwrap().into_iter().map(|m| m.folder_id).collect();
        f.sort();
        f
    };
    assert_eq!(hits(SearchScope::All), vec![a_inbox, a_work, b_inbox]);
    assert_eq!(hits(SearchScope::Account { account_id: a }), vec![a_inbox, a_work]);
    assert_eq!(hits(SearchScope::Unified { role: "inbox".into() }), vec![a_inbox, b_inbox]);
    assert_eq!(hits(SearchScope::Folder { folder_id: a_work }), vec![a_work]);
    assert!(db.search("invoice", &SearchScope::Unified { role: "bogus".into() }, 50).is_err());
}

#[test]
fn structured_queries_filter_count_and_group() {
    let db = Db::open(&temp_db()).unwrap();
    let a = db.save_account(&account()).unwrap();
    let inbox = db.upsert_folder(a, "INBOX", "Inbox", Some("inbox")).unwrap();
    let all = db.upsert_folder(a, "[Gmail]/All Mail", "All Mail", Some("all")).unwrap();
    let sent = db.upsert_folder(a, "Sent", "Sent", Some("sent")).unwrap();
    let from = |uid, name: &str, addr: &str, subject: &str, date| NewMessage {
        message_id: Some(format!("<m{uid}>")),
        from_name: name.into(),
        from_addr: addr.into(),
        subject: subject.into(),
        date,
        ..msg(uid, uid % 2 == 0)
    };
    db.insert_messages(a, inbox, &[
        from(1, "Tokopedia", "noreply@tokopedia.com", "Invoice INV/2026/11", 100),
        from(2, "Tokopedia", "noreply@tokopedia.com", "Pesanan dikirim", 200),
        NewMessage { has_attachments: true, ..from(3, "Budi", "budi@contoh.id", "Notulen rapat", 300) },
    ]).unwrap();
    // The same message again in All Mail: counted once.
    db.insert_messages(a, all, &[from(1, "Tokopedia", "noreply@tokopedia.com", "Invoice INV/2026/11", 100)]).unwrap();
    db.insert_messages(a, sent, &[NewMessage {
        to: vec![Addr { name: Some("Budi".into()), email: "budi@contoh.id".into() }],
        ..from(9, "Me", "me@example.com", "Re: Notulen rapat", 400)
    }]).unwrap();
    db.store_body(inbox, 3, b"", &BodyInfo { snippet: "".into(), search_text: "agenda anggaran kuartal".into(), has_attachments: true, attachment_names: "report.pdf".into() }).unwrap();

    let q = |q: MailQuery| db.query_mail(&q).unwrap();
    let subjects = |r: MailQueryResult| r.rows.into_iter().map(|m| m.subject).collect::<Vec<_>>();

    let r = q(MailQuery { from: Some("tokopedia".into()), ..Default::default() });
    assert_eq!(r.total, 2);
    assert_eq!(subjects(r), vec!["Pesanan dikirim", "Invoice INV/2026/11"]);
    assert_eq!(q(MailQuery { text: Some("anggaran".into()), ..Default::default() }).total, 1, "body words");
    assert_eq!(q(MailQuery { subject: Some("rapat".into()), ..Default::default() }).total, 2);
    assert_eq!(q(MailQuery { subject: Some("anggaran".into()), ..Default::default() }).total, 0, "subject only");
    assert_eq!(q(MailQuery { to: Some("budi".into()), ..Default::default() }).total, 1);
    assert_eq!(q(MailQuery { to: Some("email".into()), ..Default::default() }).total, 0, "JSON keys don't match to:");
    assert_eq!(q(MailQuery { after: Some(150), before: Some(300), ..Default::default() }).total, 1);
    assert_eq!(q(MailQuery { has_attachments: Some(true), ..Default::default() }).total, 1);
    assert_eq!(q(MailQuery { unread: Some(true), role: Some("inbox".into()), ..Default::default() }).total, 2);
    assert_eq!(q(MailQuery { role: Some("all".into()), ..Default::default() }).total, 1);
    assert_eq!(q(MailQuery { from: Some("100%_".into()), ..Default::default() }).total, 0, "LIKE wildcards are literal");
    // A server hit counts as matching the words even when the local copy has no body.
    let id = q(MailQuery { subject: Some("pesanan".into()), ..Default::default() }).rows[0].id;
    let r = q(MailQuery { text: Some("resi".into()), server_hits: vec![id], ..Default::default() });
    assert_eq!(subjects(r), vec!["Pesanan dikirim"]);
    // Paging and order.
    let r = q(MailQuery { limit: Some(2), offset: Some(1), oldest_first: true, ..Default::default() });
    assert_eq!(r.total, 4);
    assert_eq!(subjects(r), vec!["Pesanan dikirim", "Notulen rapat"]);
    assert!(db.query_mail(&MailQuery { role: Some("bogus".into()), ..Default::default() }).is_err());

    let (total, groups) = db.group_mail(&MailQuery::default(), "sender").unwrap();
    assert_eq!(total, 4);
    assert_eq!((groups[0].key.as_str(), groups[0].count, groups[0].label.as_deref()), ("noreply@tokopedia.com", 2, Some("Tokopedia")));
    let (_, groups) = db.group_mail(&MailQuery { role: Some("sent".into()), ..Default::default() }, "recipient").unwrap();
    assert_eq!((groups[0].key.as_str(), groups[0].count), ("budi@contoh.id", 1));
    let (_, groups) = db.group_mail(&MailQuery::default(), "domain").unwrap();
    assert_eq!(groups[0].key, "tokopedia.com");
    assert!(db.group_mail(&MailQuery::default(), "bogus").is_err());
}

#[test]
fn outbox_claims_and_cancels() {
    let db = Db::open(&temp_db()).unwrap();
    let acc = db.save_account(&account()).unwrap();
    let a = db.add_outbox(acc, "<1>", "{}", b"raw", "{}", 100).unwrap();
    let b = db.add_outbox(acc, "<2>", "{}", b"raw", "{}", 200).unwrap();
    assert_eq!(db.next_due().unwrap(), Some(100));

    let due = db.claim_due_outbox(150).unwrap();
    assert_eq!(due.iter().map(|r| r.id).collect::<Vec<_>>(), vec![a]);
    // Being sent: can't be taken back, and isn't claimed twice.
    assert!(db.take_outbox(a).unwrap().is_none());
    assert!(db.claim_due_outbox(150).unwrap().is_empty());
    assert_eq!(db.next_due().unwrap(), Some(200));

    db.outbox_failed(a, "nope", 300).unwrap();
    assert_eq!(db.outbox().unwrap()[1].4.as_deref(), Some("nope"));
    assert!(db.take_outbox(b).unwrap().is_some());
    assert_eq!(db.outbox().unwrap().len(), 1);
}

#[test]
fn snoozes_come_due_and_bump_the_message() {
    let db = Db::open(&temp_db()).unwrap();
    let acc = db.save_account(&account()).unwrap();
    let inbox = db.upsert_folder(acc, "INBOX", "Inbox", Some("inbox")).unwrap();
    db.insert_messages(acc, inbox, &[mail(1, "<old>", None, None, 10), mail(2, "<new>", None, None, 20)]).unwrap();

    db.add_snooze(acc, "<old>", "INBOX", 500).unwrap();
    let rows = db.list_threads(&View::Folder { folder_id: inbox }, 0, 10).unwrap();
    assert_eq!(rows.iter().find(|r| r.subject == "m1").unwrap().snoozed_until, Some(500));
    assert!(db.due_snoozes(499).unwrap().is_empty());
    let due = db.due_snoozes(500).unwrap();
    assert_eq!(due.len(), 1);
    db.mark_woke(due[0].id, 600).unwrap();

    // Back from snooze, it sorts as fresh.
    let rows = db.list_threads(&View::Folder { folder_id: inbox }, 0, 10).unwrap();
    assert_eq!(rows[0].subject, "m1");
    assert_eq!(rows[0].snoozed_until, None);
    assert_eq!(db.next_due().unwrap(), None);
}

#[test]
fn badge_counts_only_unread_inbox_mail_after_the_mark() {
    let db = Db::open(&temp_db()).unwrap();
    let acc = db.save_account(&account()).unwrap();
    let inbox = db.upsert_folder(acc, "INBOX", "Inbox", Some("inbox")).unwrap();
    let other = db.upsert_folder(acc, "Lists", "Lists", None).unwrap();

    db.insert_messages(acc, inbox, &[msg(1, false), msg(2, true)]).unwrap();
    let mark = db.max_message_id().unwrap();
    // Already-seen-in-app mail stays unread but isn't new.
    assert_eq!(db.unified_unread().unwrap(), 1);
    assert_eq!(db.unified_unread_after(mark).unwrap(), 0);

    db.insert_messages(acc, inbox, &[msg(3, false), msg(4, true)]).unwrap();
    db.insert_messages(acc, other, &[msg(5, false)]).unwrap();
    assert_eq!(db.unified_unread_after(mark).unwrap(), 1);
    assert_eq!(db.unified_unread_after(db.max_message_id().unwrap()).unwrap(), 0);
}

#[test]
fn fresh_database_is_current_and_not_backed_up() {
    let path = temp_db();
    Db::open(&path).unwrap();
    assert_eq!(version(&path), SCHEMA_VERSION);
    assert!(!path.with_extension("v0.bak").exists());
}

#[test]
fn reopening_keeps_data() {
    let path = temp_db();
    Db::open(&path).unwrap().save_account(&account()).unwrap();
    let db = Db::open(&path).unwrap();
    let accs = db.accounts().unwrap();
    assert_eq!(accs.len(), 1);
    assert_eq!(accs[0].signature, "<b>Me</b>");
    assert_eq!(accs[0].label, "Work");
}

#[test]
fn unversioned_database_from_first_build_is_adopted_with_backup() {
    let path = temp_db();
    // A database written before versioning existed: v1 tables, user_version 0.
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(V1).unwrap();
        conn.execute(
            "INSERT INTO accounts (name, email, imap_host, imap_port, smtp_host, smtp_port) VALUES ('Me', 'me@example.com', 'i', 993, 's', 465)",
            [],
        )
        .unwrap();
    }

    let db = Db::open(&path).unwrap();
    assert_eq!(version(&path), SCHEMA_VERSION);
    assert_eq!(db.accounts().unwrap().len(), 1);
    assert!(path.with_extension("v0.bak").exists());
}

#[test]
fn future_migration_upgrades_existing_data_after_backup() {
    let path = temp_db();
    Db::open(&path).unwrap().save_account(&account()).unwrap();

    let next = "ALTER TABLE accounts ADD COLUMN nickname TEXT NOT NULL DEFAULT 'none';";
    let mut conn = Connection::open(&path).unwrap();
    let mut all: Vec<&str> = MIGRATIONS.to_vec();
    all.push(next);
    migrate_with(&mut conn, &path, &all).unwrap();

    assert_eq!(version(&path), SCHEMA_VERSION + 1);
    let nick: String = conn.query_row("SELECT nickname FROM accounts", [], |r| r.get(0)).unwrap();
    assert_eq!(nick, "none");
    let email: String = conn.query_row("SELECT email FROM accounts", [], |r| r.get(0)).unwrap();
    assert_eq!(email, "me@example.com");
    // The pre-upgrade backup holds the old schema and the data.
    let bak = Connection::open(path.with_extension(format!("v{SCHEMA_VERSION}.bak"))).unwrap();
    let v: i64 = bak.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert_eq!(v, SCHEMA_VERSION);
    let n: i64 = bak.query_row("SELECT COUNT(*) FROM accounts", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 1);
}

#[test]
fn v1_database_gains_label_and_keeps_accounts() {
    let path = temp_db();
    {
        let mut conn = Connection::open(&path).unwrap();
        migrate_with(&mut conn, &path, &[V1]).unwrap();
        conn.execute(
            "INSERT INTO accounts (name, email, imap_host, imap_port, smtp_host, smtp_port) VALUES ('Me', 'me@example.com', 'i', 993, 's', 465)",
            [],
        )
        .unwrap();
    }
    let db = Db::open(&path).unwrap();
    let accs = db.accounts().unwrap();
    assert_eq!(accs.len(), 1);
    assert_eq!(accs[0].label, "");
    assert_eq!(version(&path), SCHEMA_VERSION);
    assert!(path.with_extension("v1.bak").exists());
}

#[test]
fn folder_and_account_order_persist() {
    let path = temp_db();
    let db = Db::open(&path).unwrap();
    let a1 = db.save_account(&account()).unwrap();
    let a2 = db.save_account(&Account { email: "two@example.com".into(), ..account() }).unwrap();
    let inbox = db.upsert_folder(a1, "INBOX", "Inbox", Some("inbox")).unwrap();
    let sent = db.upsert_folder(a1, "Sent", "Sent", Some("sent")).unwrap();
    let news = db.upsert_folder(a1, "News", "News", None).unwrap();
    let b_inbox = db.upsert_folder(a2, "INBOX", "Inbox", Some("inbox")).unwrap();

    // Default: role order, accounts in creation order.
    let ids: Vec<i64> = db.folders().unwrap().iter().map(|f| f.id).collect();
    assert_eq!(ids, vec![inbox, sent, news, b_inbox]);

    db.reorder_folders(a1, &[news, inbox, sent]).unwrap();
    db.reorder_accounts(&[a2, a1]).unwrap();
    // A re-sync upserts folders again; the order must survive it.
    db.upsert_folder(a1, "News", "News", None).unwrap();

    let ids: Vec<i64> = Db::open(&path).unwrap().folders().unwrap().iter().map(|f| f.id).collect();
    assert_eq!(ids, vec![b_inbox, news, inbox, sent]);
    // Another account's ids are ignored.
    db.reorder_folders(a2, &[b_inbox, news]).unwrap();
    assert_eq!(db.folders().unwrap().iter().find(|f| f.id == news).unwrap().position, Some(0));
}

#[test]
fn v4_database_gains_rules_and_keeps_mail() {
    let path = temp_db();
    {
        let mut conn = Connection::open(&path).unwrap();
        migrate_with(&mut conn, &path, &[V1, V2, V3, V4]).unwrap();
        conn.execute("INSERT INTO accounts (name, email, imap_host, imap_port, smtp_host, smtp_port) VALUES ('Me', 'me@x.com', 'i', 993, 's', 465)", []).unwrap();
        conn.execute("INSERT INTO folders (account_id, path, name, role) VALUES (1, 'INBOX', 'INBOX', 'inbox')", []).unwrap();
        conn.execute("INSERT INTO messages (account_id, folder_id, uid, subject) VALUES (1, 1, 1, 'Old')", []).unwrap();
    }
    let db = Db::open(&path).unwrap();
    assert_eq!(version(&path), SCHEMA_VERSION);
    let f = db.rule_facts(1).unwrap().unwrap();
    assert_eq!((f.subject.as_str(), f.list_id, f.auto), ("Old", None, None));
    assert!(db.rules().unwrap().is_empty());
    // New mail stores the headers rules use.
    let m = NewMessage { list_id: Some("<l.x.com>".into()), auto: Some("auto-replied".into()), ..msg(2, false) };
    let ids = db.insert_messages(1, 1, &[m]).unwrap();
    let f = db.rule_facts(ids[0]).unwrap().unwrap();
    assert_eq!((f.list_id.as_deref(), f.auto.as_deref()), (Some("<l.x.com>"), Some("auto-replied")));
    assert!(db.insert_messages(1, 1, &[msg(2, false)]).unwrap().is_empty());
}

#[test]
fn failed_migration_rolls_back() {
    let path = temp_db();
    Db::open(&path).unwrap().save_account(&account()).unwrap();
    let bad = "ALTER TABLE accounts ADD COLUMN x TEXT; THIS IS NOT SQL;";
    let mut conn = Connection::open(&path).unwrap();
    let mut all: Vec<&str> = MIGRATIONS.to_vec();
    all.push(bad);
    assert!(migrate_with(&mut conn, &path, &all).is_err());
    assert_eq!(version(&path), SCHEMA_VERSION);
    let cols: i64 = conn.query_row("SELECT COUNT(*) FROM pragma_table_info('accounts') WHERE name = 'x'", [], |r| r.get(0)).unwrap();
    assert_eq!(cols, 0);
}

#[test]
fn newer_database_is_refused_untouched() {
    let path = temp_db();
    Db::open(&path).unwrap().save_account(&account()).unwrap();
    Connection::open(&path).unwrap().pragma_update(None, "user_version", SCHEMA_VERSION + 1).unwrap();

    let err = Db::open(&path).err().expect("should refuse").to_string();
    assert!(err.contains("newer version"));
    assert_eq!(version(&path), SCHEMA_VERSION + 1);
}

#[test]
fn contacts_escapes_wildcards_and_folder_synced_tracks_empty_inbox() {
    let db = Db::open(&temp_db()).unwrap();
    let a = db.save_account(&account()).unwrap();
    let f = db.upsert_folder(a, "INBOX", "Inbox", Some("inbox")).unwrap();
    assert!(!db.folder_synced(f).unwrap());
    db.mark_folder_synced(f).unwrap();
    assert!(db.folder_synced(f).unwrap());
    db.reset_folder(f, 42).unwrap();
    assert!(!db.folder_synced(f).unwrap());

    let m1 = NewMessage { from_name: "A_B".into(), from_addr: "a_b@example.com".into(), ..msg(1, false) };
    let m2 = NewMessage { from_name: "AXB".into(), from_addr: "axb@example.com".into(), ..msg(2, false) };
    db.insert_messages(a, f, &[m1, m2]).unwrap();
    let hits = db.contacts("a_b").unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].email, "a_b@example.com");
}

#[test]
fn sent_recipients_collects_unique_lowercased_to_and_cc_from_sent_only() {
    let db = Db::open(&temp_db()).unwrap();
    let a = db.save_account(&account()).unwrap();
    let inbox = db.upsert_folder(a, "INBOX", "Inbox", Some("inbox")).unwrap();
    let sent = db.upsert_folder(a, "Sent", "Sent", Some("sent")).unwrap();

    let inbox_msg = NewMessage {
        to: vec![Addr { name: None, email: "ignored@example.com".into() }],
        cc: vec![Addr { name: None, email: "ignored-cc@example.com".into() }],
        ..msg(1, false)
    };
    let sent_msg1 = NewMessage {
        to: vec![
            Addr { name: Some("Alice".into()), email: "Alice@Example.com".into() },
            Addr { name: None, email: "".into() },
        ],
        cc: vec![Addr { name: None, email: "Bob@Work.org".into() }],
        ..msg(2, true)
    };
    let sent_msg2 = NewMessage {
        to: vec![Addr { name: None, email: "alice@example.com".into() }],
        cc: vec![],
        ..msg(3, true)
    };
    db.insert_messages(a, inbox, &[inbox_msg]).unwrap();
    db.insert_messages(a, sent, &[sent_msg1, sent_msg2]).unwrap();

    let mut got = db.sent_recipients().unwrap();
    got.sort();
    assert_eq!(got, vec!["alice@example.com".to_string(), "bob@work.org".to_string()]);
}

#[test]
fn v6_database_gains_agent_tables_and_cascades_on_account_delete() {
    let path = temp_db();
    {
        let mut conn = Connection::open(&path).unwrap();
        migrate_with(&mut conn, &path, &[V1, V2, V3, V4, V5, V6]).unwrap();
        conn.execute(
            "INSERT INTO accounts (name, email, imap_host, imap_port, smtp_host, smtp_port) VALUES ('Me', 'me@x.com', 'i', 993, 's', 465)",
            [],
        )
        .unwrap();
    }
    let db = Db::open(&path).unwrap();
    assert_eq!(version(&path), SCHEMA_VERSION);
    db.set_agent_account_mode(1, Some(AgentMode::Auto)).unwrap();
    assert_eq!(db.agent_account_modes().unwrap().get(&1), Some(&AgentMode::Auto));
    let spec = serde_json::to_string(&AgentActionSpec::Archive { ids: vec![1] }).unwrap();
    let preview = serde_json::to_string(&AgentActionPreview::default()).unwrap();
    let id = db.insert_agent_action(Some(1), "archive", "Archive 1 message", &spec, &preview, "pending", 100, None).unwrap();
    assert_eq!(db.pending_agent_actions_count().unwrap(), 1);
    assert!(db.agent_action(id).unwrap().is_some());

    // Deleting the account cascades to both tables.
    db.delete_account(1).unwrap();
    assert!(db.agent_account_modes().unwrap().is_empty());
    assert_eq!(db.pending_agent_actions_count().unwrap(), 0);
}

#[test]
fn v7_upgrades_to_v8_and_persists_sync_state_and_pending_ops() {
    let path = temp_db();
    {
        let mut conn = Connection::open(&path).unwrap();
        migrate_with(&mut conn, &path, &[V1, V2, V3, V4, V5, V6, V7]).unwrap();
        conn.execute(
            "INSERT INTO accounts (name, email, imap_host, imap_port, smtp_host, smtp_port) VALUES ('Me', 'me@x.com', 'i', 993, 's', 465)",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO folders (account_id, path, name, role, uidvalidity, synced) VALUES (1, 'INBOX', 'Inbox', 'inbox', 7, 1)", [])
            .unwrap();
    }
    let db = Db::open(&path).unwrap();
    assert_eq!(version(&path), SCHEMA_VERSION);
    let st = db.folder_sync_state(1).unwrap();
    assert_eq!((st.uidvalidity, st.synced, st.msg_count, st.highest_modseq), (7, true, -1, 0));

    let next_st = FolderSyncState {
        uidvalidity: 7,
        synced: true,
        uidnext: 50,
        msg_count: 12,
        unseen_count: 3,
        highest_modseq: 999,
        last_full_sync: 1000,
    };
    db.update_folder_sync_state(1, &next_st).unwrap();
    assert_eq!(db.folder_sync_state(1).unwrap(), next_st);

    let op_id = db
        .enqueue_pending_op(1, 1, &PendingOpPayload::Store { flag: "\\Seen".into(), add: true }, &[10, 11], 100)
        .unwrap();
    assert!(db.has_pending_ops_for_folder(1).unwrap());
    let (pending_uids, mark_all) = db.pending_uids_for_folder(1).unwrap();
    assert!(!mark_all && pending_uids.contains(&10) && pending_uids.contains(&11));
    assert_eq!(db.due_pending_ops(1, Some(1), 100).unwrap().len(), 1);
    db.fail_pending_op(op_id, 200).unwrap();
    assert!(db.due_pending_ops(1, Some(1), 150).unwrap().is_empty());
    assert_eq!(db.due_pending_ops(1, Some(1), 200).unwrap()[0].attempts, 1);
    db.delete_pending_op(op_id).unwrap();
    assert!(!db.has_pending_ops_for_folder(1).unwrap());
}

#[test]
fn reconciliation_protects_pending_ops_and_computes_missing_uids() {
    let db = Db::open(&temp_db()).unwrap();
    let acc = db.save_account(&account()).unwrap();
    let inbox = db.upsert_folder(acc, "INBOX", "Inbox", Some("inbox")).unwrap();
    // Local has UID 1 (optimistically marked seen), UID 2 (unseen), UID 3 (removed on server).
    // UID 4 was optimistically archived locally (in pending_uids), UID 5 is genuinely missing locally.
    db.insert_messages(acc, inbox, &[msg(1, true), msg(2, false), msg(3, false)]).unwrap();
    let pending: std::collections::HashSet<u32> = [1, 4].into_iter().collect();
    let server_flags = vec![(1, false, false), (2, true, true), (4, false, false), (5, false, false)];

    let (changed, missing) = db.reconcile_flags_and_missing(inbox, 1, &server_flags, &pending, false).unwrap();
    assert!(changed);
    // UID 4 is pending move -> must NOT be resurrected; UID 5 is missing -> returned for healing.
    assert_eq!(missing, vec![5]);
    // UID 1 stays seen=true despite server saying false; UID 2 updated to seen=true, flagged=true; UID 3 deleted.
    let rows = db.list(&View::Folder { folder_id: inbox }, None, 0, 10).unwrap();
    assert_eq!(rows.len(), 2);
    let r1 = rows.iter().find(|r| r.subject == "m1").unwrap();
    let r2 = rows.iter().find(|r| r.subject == "m2").unwrap();
    assert!(r1.seen);
    assert!(r2.seen && r2.flagged);
}

#[test]
fn condstore_apply_changed_and_existing_uids_work() {
    let db = Db::open(&temp_db()).unwrap();
    let acc = db.save_account(&account()).unwrap();
    let inbox = db.upsert_folder(acc, "INBOX", "Inbox", Some("inbox")).unwrap();
    db.insert_messages(acc, inbox, &[msg(10, false), msg(11, false), msg(12, false)]).unwrap();
    let empty = std::collections::HashSet::new();

    assert!(db.apply_changed_flags(inbox, &[(11, true, true)], &empty, false).unwrap());
    let present: std::collections::HashSet<u32> = [10, 11, 13].into_iter().collect();
    let (changed, missing) = db.reconcile_existing_uids(inbox, 10, &present, &empty).unwrap();
    assert!(changed);
    assert_eq!(missing, vec![13]);
    let rows = db.list(&View::Folder { folder_id: inbox }, None, 0, 10).unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().find(|r| r.subject == "m11").unwrap().flagged);
}

#[test]
fn bodies_dedup_across_folders_stash_on_move_and_prune_by_size() {
    let db = Db::open(&temp_db()).unwrap();
    let acc = db.save_account(&account()).unwrap();
    let inbox = db.upsert_folder(acc, "INBOX", "Inbox", Some("inbox")).unwrap();
    let archive = db.upsert_folder(acc, "Archive", "Archive", Some("archive")).unwrap();
    let label = db.upsert_folder(acc, "Work", "Work", None).unwrap();

    let ids = db.insert_messages(acc, inbox, &[mail(1, "<m1>", None, None, 100)]).unwrap();
    let raw = vec![b'x'; 1024];
    let info = BodyInfo { snippet: "hello".into(), search_text: "hello world".into(), has_attachments: false, ..Default::default() };
    db.store_bodies_batch(inbox, &[(1, raw.clone(), info)]).unwrap();

    // Cross-folder dedup (Gmail labels): Work has same Message-ID -> hydrates locally.
    db.insert_messages(acc, label, &[mail(7, "<m1>", None, None, 100)]).unwrap();
    assert_eq!(db.copy_existing_bodies_for_folder(acc, label).unwrap(), 1);
    let label_id = db.ids_for_uids(label, &[7]).unwrap()[0];
    assert_eq!(db.body(label_id).unwrap().unwrap(), raw);

    // Stash on move: deleting from Inbox preserves body for Archive copy.
    db.stash_moved_bodies(&ids, 500).unwrap();
    db.delete_messages(&ids).unwrap();
    db.insert_messages(acc, archive, &[mail(9, "<m1>", None, None, 100)]).unwrap();
    assert_eq!(db.copy_existing_bodies_for_folder(acc, archive).unwrap(), 1);
    let arch_id = db.ids_for_uids(archive, &[9]).unwrap()[0];
    assert_eq!(db.body(arch_id).unwrap().unwrap(), raw);

    // Cross-folder dedup (Gmail labels): Work shares the single compressed raw_blobs row.
    let blob_count: i64 = db.r().query_row("SELECT COUNT(*) FROM raw_blobs WHERE account_id = ?", [acc], |r| r.get(0)).unwrap();
    assert_eq!(blob_count, 1);
    let stored_len: i64 = db.r().query_row("SELECT LENGTH(raw) FROM raw_blobs WHERE account_id = ?", [acc], |r| r.get(0)).unwrap();
    assert!(stored_len < raw.len() as i64, "expected compressed blob smaller than raw");

    // Prune evicts oldest bodies when account exceeds max_bytes (keeping snippet/FTS)
    // and marks has_body = 2 so background prefetch never re-downloads evicted rows.
    assert!(db.prune_old_bodies(acc, 8).unwrap() >= 1);
    assert!(db.body(arch_id).unwrap().is_none());
    assert_eq!(db.body_text(arch_id).unwrap().as_deref(), Some("hello world"));
    assert!(db.missing_bodies_with_size(archive, 100, 4 * 1024 * 1024).unwrap().is_empty());
}

#[test]
fn prefetch_skips_attachments_and_oversized_messages() {
    let db = Db::open(&temp_db()).unwrap();
    let acc = db.save_account(&account()).unwrap();
    let inbox = db.upsert_folder(acc, "INBOX", "Inbox", Some("inbox")).unwrap();

    let mut plain = mail(1, "<p1>", None, None, 100);
    plain.size = 50_000;
    let mut with_att_hdr = mail(2, "<a2>", None, None, 200);
    with_att_hdr.has_attachments = true;
    with_att_hdr.size = 50_000;
    let mut oversized = mail(3, "<big3>", None, None, 300);
    oversized.size = 4 * 1024 * 1024 + 1;
    let mut hidden_att = mail(4, "<ha4>", None, None, 400);
    hidden_att.size = 100_000;

    let ids = db.insert_messages(acc, inbox, &[plain, with_att_hdr, oversized, hidden_att]).unwrap();
    let missing = db.missing_bodies_with_size(inbox, 100, 4 * 1024 * 1024).unwrap();
    // UID 2 (has_attachments=1) and UID 3 (>4MB) are excluded; UID 1 and UID 4 are eligible.
    assert_eq!(missing.iter().map(|(u, _)| *u).collect::<Vec<_>>(), vec![4, 1]);

    // If UID 4 turns out to have attachments on parse, store_bodies_batch does NOT store its body.
    let stored = db
        .store_bodies_batch(
            inbox,
            &[
                (1, b"plain body".to_vec(), BodyInfo { snippet: "plain".into(), search_text: "plain".into(), has_attachments: false, ..Default::default() }),
                (4, b"has pdf".to_vec(), BodyInfo { snippet: "pdf".into(), search_text: "pdf".into(), has_attachments: true, attachment_names: "doc.pdf".into() }),
            ],
        )
        .unwrap();
    assert_eq!(stored, 1);
    assert!(db.body(ids[0]).unwrap().is_some());
    assert!(db.body(ids[3]).unwrap().is_none());
    // And future prefetch queries now skip UID 4 too.
    assert!(db.missing_bodies_with_size(inbox, 100, 4 * 1024 * 1024).unwrap().is_empty());
}

#[test]
fn quiet_messages_are_excluded_from_new_badge_count() {
    let db = Db::open(&temp_db()).unwrap();
    let acc = db.save_account(&account()).unwrap();
    let inbox = db.upsert_folder(acc, "INBOX", "Inbox", Some("inbox")).unwrap();
    let mark = db.max_message_id().unwrap();
    let ids = db.insert_messages(acc, inbox, &[msg(1, false), msg(2, false)]).unwrap();
    assert_eq!(db.unified_unread_after(mark).unwrap(), 2);
    db.mark_quiet(ids[0]).unwrap();
    assert_eq!(db.unified_unread_after(mark).unwrap(), 1);
}

#[test]
fn v9_aliases_reply_to_ics_rsvps_smart_views_and_folder_crud() {
    let db = Db::open(&temp_db()).unwrap();
    let mut a = account();
    a.aliases = vec![Alias { name: "Support".into(), email: "support@example.com".into() }];
    a.reply_to = "replies@example.com".into();
    let acc = db.save_account(&a).unwrap();
    let loaded = db.account(acc).unwrap();
    assert_eq!(loaded.aliases, a.aliases);
    assert_eq!(loaded.reply_to, "replies@example.com");

    // ICS RSVP persistence
    assert_eq!(db.ics_rsvp(acc, "evt-1").unwrap(), None);
    db.set_ics_rsvp(acc, "evt-1", "ACCEPTED", 1000).unwrap();
    assert_eq!(db.ics_rsvp(acc, "evt-1").unwrap().as_deref(), Some("ACCEPTED"));
    db.set_ics_rsvp(acc, "evt-1", "DECLINED", 2000).unwrap();
    assert_eq!(db.ics_rsvp(acc, "evt-1").unwrap().as_deref(), Some("DECLINED"));

    // Smart views: Unread & Flagged exclude trash/junk and deduplicate All Mail
    let inbox = db.upsert_folder(acc, "INBOX", "Inbox", Some("inbox")).unwrap();
    let trash = db.upsert_folder(acc, "Trash", "Trash", Some("trash")).unwrap();
    let custom = db.upsert_folder(acc, "Projects", "Projects", None).unwrap();
    let sub = db.upsert_folder(acc, "Projects/Alpha", "Alpha", None).unwrap();

    let mut m1 = msg(1, false);
    m1.flagged = true;
    let m2 = msg(2, true);
    let mut m3 = msg(3, false);
    m3.flagged = true;
    db.insert_messages(acc, inbox, &[m1, m2]).unwrap();
    db.insert_messages(acc, trash, &[m3]).unwrap();

    let counts = db.smart_counts().unwrap();
    assert_eq!(counts.flagged, 1);
    assert_eq!(counts.unread, 1);
    assert_eq!(db.list_threads(&View::Smart { name: "flagged".into(), account_id: None }, 0, 50).unwrap().len(), 1);
    assert_eq!(db.list_threads(&View::Smart { name: "unread".into(), account_id: None }, 0, 50).unwrap().len(), 1);

    // Folder rename cascades child paths, clear_folder_messages empties Trash, delete removes folder
    db.rename_folder_local(custom, "Clients", "Clients").unwrap();
    assert_eq!(db.folder(custom).unwrap().1, "Clients");
    assert_eq!(db.folder(sub).unwrap().1, "Clients/Alpha");

    db.clear_folder_messages(trash).unwrap();
    assert!(db.list(&View::Folder { folder_id: trash }, None, 0, 50).unwrap().is_empty());

    db.delete_folder_local(sub).unwrap();
    assert!(db.folder(sub).is_err());
}

#[test]
fn repair_html_snippets_fixes_cached_html_and_preserves_plain_code() {
    let path = temp_db();
    let html_raw = concat!(
        "From: Redhub <redhub@example.com>\r\n",
        "Subject: Reminder\r\n",
        "Content-Type: text/plain; charset=utf-8\r\n\r\n",
        "<!DOCTYPE html><html><head><style>p{color:red}</style></head>",
        "<body><p>Dear Semangat!,</p><p>Upload dokumen hari ini.</p></body></html>",
    )
    .as_bytes();
    let code_raw = concat!(
        "From: Dev <dev@example.com>\r\n",
        "Subject: Review\r\n",
        "Content-Type: text/plain; charset=utf-8\r\n\r\n",
        "Use Vec<String> instead of <div>.\r\n",
    )
    .as_bytes();

    let (inbox, id_html, id_code) = {
        let db = Db::open(&path).unwrap();
        let acc = db.save_account(&account()).unwrap();
        let inbox = db.upsert_folder(acc, "INBOX", "Inbox", Some("inbox")).unwrap();
        let ids = db.insert_messages(acc, inbox, &[msg(1, false), msg(2, false)]).unwrap();
        // Simulate legacy stored snippets before the fix (raw HTML in snippet + FTS).
        db.store_body(
            inbox,
            1,
            html_raw,
            &BodyInfo {
                snippet: "<!DOCTYPE html><html><head><style>p{color:red}</style></head>".into(),
                search_text: "<!DOCTYPE html><html>".into(),
                has_attachments: false,
                ..Default::default()
            },
        )
        .unwrap();
        db.store_body(
            inbox,
            2,
            code_raw,
            &BodyInfo {
                snippet: "Use Vec<String> instead of <div>.".into(),
                search_text: "Use Vec<String> instead of <div>.".into(),
                has_attachments: false,
                ..Default::default()
            },
        )
        .unwrap();
        // Clear the one-time repair marker to simulate upgrading from an older version.
        db.delete_setting("snippets_html_repaired_v1").unwrap();
        (inbox, ids[0], ids[1])
    };

    // Reopening runs the one-time repair pass.
    let db = Db::open(&path).unwrap();
    let list = db.list(&View::Folder { folder_id: inbox }, None, 0, 10).unwrap();
    let by_id = |id| list.iter().find(|m| m.id == id).unwrap().snippet.clone();
    assert_eq!(by_id(id_html), "Dear Semangat!, Upload dokumen hari ini.");
    assert_eq!(by_id(id_code), "Use Vec<String> instead of <div>.");
    // FTS body was updated too.
    let fts_hits = db.search("Semangat", &SearchScope::Folder { folder_id: inbox }, 10).unwrap();
    assert_eq!(fts_hits.len(), 1);
    assert_eq!(fts_hits[0].id, id_html);
}

#[test]
fn boolean_search_filename_size_saved_searches_and_mcp_allowlist_work() {
    let db = Db::open(&temp_db()).unwrap();
    let acc = db.save_account(&account()).unwrap();
    let inbox = db.upsert_folder(acc, "INBOX", "Inbox", Some("inbox")).unwrap();
    let spam = db.upsert_folder(acc, "Spam", "Spam", Some("junk")).unwrap();
    let all_mail = db.upsert_folder(acc, "[Gmail]/All Mail", "All Mail", Some("all")).unwrap();

    let mut m1 = msg(1, false);
    m1.message_id = Some("<inv1@x>".into());
    m1.from_addr = "billing@stripe.com".into();
    m1.subject = "Quarterly report invoice".into();
    m1.size = 6 * 1024 * 1024;
    m1.has_attachments = true;

    let mut m2 = msg(2, true);
    m2.message_id = Some("<rec2@x>".into());
    m2.from_addr = "ana@acme.co".into();
    m2.subject = "Receipt for coffee".into();
    m2.size = 50 * 1024;

    let mut m3 = msg(3, false);
    m3.message_id = Some("<spam3@x>".into());
    m3.from_addr = "promo@spam.example".into();
    m3.subject = "Invoice urgent prize".into();
    m3.size = 10 * 1024;

    db.insert_messages(acc, inbox, &[m1, m2]).unwrap();
    db.insert_messages(acc, spam, &[m3.clone()]).unwrap();
    // Gmail-style copy of spam in All Mail.
    db.insert_messages(acc, all_mail, &[NewMessage { uid: 99, ..m3 }]).unwrap();
    db.store_body(
        inbox,
        1,
        b"",
        &BodyInfo {
            snippet: "Q4".into(),
            search_text: "quarterly report attached".into(),
            has_attachments: true,
            attachment_names: "Q4-invoice.pdf\nsummary.xlsx".into(),
        },
    )
    .unwrap();

    let run = |q: &str| {
        let p = crate::search::parse_search_query(q, &SearchScope::All);
        db.search_expr(&p.expr.unwrap(), &SearchScope::All, p.has_in_scope, 50).unwrap()
    };
    assert_eq!(run("invoice OR receipt").len(), 3);
    assert_eq!(run("(invoice OR receipt) -from:spam.example").len(), 2);
    assert_eq!(run(r#""quarterly report""#).len(), 1);
    assert_eq!(run("filename:pdf").len(), 1);
    assert_eq!(run("filename:xlsx").len(), 1);
    assert_eq!(run("larger:5M").len(), 1);
    assert_eq!(run("smaller:100K -from:spam.example").len(), 1);

    // Saved searches CRUD + reorder + unread count.
    let s1 = db.save_saved_search(0, "Invoices", "invoice -from:spam.example", &SearchScope::All).unwrap();
    let s2 = db.save_saved_search(0, "Receipts", "receipt", &SearchScope::All).unwrap();
    db.reorder_saved_searches(&[s2.id, s1.id]).unwrap();
    let list = db.saved_searches().unwrap();
    assert_eq!(list.iter().map(|s| s.id).collect::<Vec<_>>(), vec![s2.id, s1.id]);
    let p = crate::search::parse_search_query(&s1.query, &s1.scope);
    assert_eq!(db.count_search_expr_unread(&p.expr.unwrap(), &s1.scope, p.has_in_scope).unwrap(), 1);
    db.delete_saved_search(s2.id).unwrap();
    assert_eq!(db.saved_searches().unwrap().len(), 1);

    // MCP allowlist excludes spam AND its All Mail copy.
    let q = MailQuery {
        text: Some("invoice".into()),
        allowed_folders: Some(vec![inbox, all_mail]),
        ..Default::default()
    };
    let r = db.query_mail(&q).unwrap();
    assert_eq!(r.total, 1);
    assert_eq!(r.rows[0].from_addr, "billing@stripe.com");
}
