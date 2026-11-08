//! Confirmed Gmail send and in-place sent-note finalization against fake gog.

mod gmail_support;

use std::{fs, path::Path, process::Output};

use gmail_support::{
    json_stdout, run_gmail, run_gmail_stdin, stderr, stdout, FakeGog, GmailEnv,
    ACCOUNT, HOST,
};
use serde_json::{json, Value};

fn assert_success(output: &Output, label: &str) {
    assert_eq!(
        output.status.code(),
        Some(0),
        "{label} expected exit 0:\nstdout:\n{}\nstderr:\n{}",
        stdout(output),
        stderr(output)
    );
}

fn calls_for(fake: &FakeGog, operation: &str) -> Vec<Vec<String>> {
    fake.calls()
        .into_iter()
        .filter(|call| {
            call.windows(3)
                .any(|items| items == ["gmail", "drafts", operation])
        })
        .collect()
}

fn create_compose(env: &GmailEnv, fake: &FakeGog, title: &str) -> String {
    let created = run_gmail(
        env,
        fake,
        &["new", "-t", "ada@example.com", "-s", title, "-f", "json"],
    );
    assert_success(&created, "create compose note");
    json_stdout(&created)["note"]
        .as_str()
        .expect("note path")
        .to_string()
}

fn make_saved_thread(env: &GmailEnv, fake: &FakeGog, path: &str) {
    let target = serde_json::json!({
        "id": "0123456789abcdef",
        "threadId": "0123456789abcdef",
        "labelIds": ["INBOX"],
        "internalDate": "1793908800000",
        "payload": {
            "mimeType": "text/plain",
            "headers": [
                {"name": "From", "value": "Dana <dana@example.test>"},
                {"name": "To", "value": "you@example.com"},
                {"name": "Cc", "value": "Lee <lee@example.test>"},
                {"name": "Subject", "value": "Re: Lease renewal"},
                {"name": "Message-ID", "value": "<lease@example.test>"}
            ],
            "body": {"size": 5, "data": "SGVsbG8="}
        }
    });
    fake.set_thread(
        serde_json::json!({
            "thread": {"id": "0123456789abcdef", "messages": [target]}
        }),
        serde_json::json!({}),
    );
    let note_path = env.vault().join(path);
    fs::create_dir_all(note_path.parent().unwrap()).unwrap();
    fs::write(
        note_path,
        "---\nparent: \"[[mail]]\"\ntype: \"[[mail]]\"\ncontent: untrusted-email\ngmail_account: you@example.com\ngmail_thread_id: 0123456789abcdef\ntitle: Lease renewal\nwho: Dana\nstarted: 2026-11-05T10:00:00+00:00\nupdated: 2026-11-05T10:00:00+00:00\nmessages: 1\n---\n# Lease renewal\n\n· Dana ↔ you · 1 message · Nov 5\n\n## Notes\n\nKeep this private.\n\n## Conversation\n\n<!-- bob:gmail:begin v1 -->\n<!-- bob:gmail:msg 0123456789abcdef %3Clease%40example.test%3E -->\n> [!mail+] **Dana** · Nov 5\n> Hello\n^m-0123456789abcdef\n<!-- bob:gmail:end -->\n",
    )
    .unwrap();
}

#[test]
fn new_preview_requires_exact_digest_and_promotes_in_place() {
    let env = GmailEnv::new("bob-cli-gmail-send-new");
    let fake = FakeGog::new(&env, "gog").scenario("compose_grant");
    let note = create_compose(&env, &fake, "Lease renewal");
    let note_path = env.vault().join(&note);
    let original = fs::read_to_string(&note_path).unwrap();
    let authored = original.replace(
        "subject: Lease renewal",
        "custom_owner: keep-me\nsubject: Lease renewal",
    );
    fs::write(
        &note_path,
        format!("{authored}Hello **Ada**.\n\n## Notes\n\nPrivate note.\n"),
    )
    .unwrap();

    let preview = run_gmail(&env, &fake, &["send", &note, "-d", "-f", "json"]);
    assert_success(&preview, "preview send");
    let preview_doc = json_stdout(&preview);
    assert_eq!(preview_doc["status"], "preview");
    let digest = preview_doc["digest"].as_str().unwrap();
    assert_eq!(preview_doc["preview"]["to"][0], "ada@example.com");
    assert_eq!(
        preview_doc["preview"]["text_body"],
        "Hello Ada.\n\n-- \nTest signature"
    );
    assert!(calls_for(&fake, "send").is_empty());

    let required = run_gmail(&env, &fake, &["send", &note, "-f", "json"]);
    assert_eq!(required.status.code(), Some(2), "{}", stderr(&required));
    let required_doc = json_stdout(&required);
    assert_eq!(required_doc["error"]["kind"], "confirmation_required");
    assert!(required_doc["error"]["hint"]
        .as_str()
        .unwrap()
        .contains(digest));
    assert!(calls_for(&fake, "send").is_empty());

    let mismatch = run_gmail(
        &env,
        &fake,
        &["send", &note, "-c", &"0".repeat(64), "-f", "json"],
    );
    assert_eq!(mismatch.status.code(), Some(2), "{}", stderr(&mismatch));
    assert_eq!(
        json_stdout(&mismatch)["error"]["kind"],
        "confirmation_mismatch"
    );
    assert!(calls_for(&fake, "send").is_empty());

    let sent =
        run_gmail(&env, &fake, &["send", &note, "-c", digest, "-f", "json"]);
    assert_success(&sent, "confirmed send");
    let sent_doc = json_stdout(&sent);
    assert_eq!(sent_doc["status"], "sent");
    assert_eq!(sent_doc["thread_id"], "0123456789abcdef");
    let sent_id = sent_doc["sent_message_id"].as_str().unwrap();

    let promoted = fs::read_to_string(&note_path).unwrap();
    assert!(promoted.contains("custom_owner: keep-me"));
    assert!(promoted.contains("gmail_thread_id: 0123456789abcdef"));
    assert!(promoted.contains("Private note."));
    assert!(promoted.contains("> [!mail-sent+] **You**"));
    assert!(!promoted.contains("status: draft"));
    assert!(!promoted.contains("gmail_draft_id:"));
    assert!(!promoted.contains("gmail_draft_remote_fingerprint:"));
    assert_eq!(
        fs::read_dir(env.vault().join("mail/2026"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry
                .path()
                .extension()
                .is_some_and(|ext| ext == "md"))
            .count(),
        1,
        "promotion must not allocate a second mail note"
    );

    let sent_calls = calls_for(&fake, "send");
    assert_eq!(sent_calls.len(), 1);
    assert!(sent_calls[0].contains(&"gmail.drafts.send".to_string()));
    assert!(fake
        .calls()
        .iter()
        .all(|call| !call.iter().any(|arg| arg == "gmail.send")));

    let journal =
        fs::read_to_string(env.vault().join(".bob/gmail/test-host.jsonl"))
            .unwrap();
    let sent_events: Vec<Value> = journal
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .filter(|event: &Value| event["kind"] == "gmail.sent")
        .collect();
    assert_eq!(sent_events.len(), 1);
    assert_eq!(sent_events[0]["value"]["thread_id"], "0123456789abcdef");
    assert_eq!(sent_events[0]["value"]["message_id"], sent_id);
    assert_eq!(sent_events[0]["value"]["recipient_count"], 1);
    assert_eq!(
        sent_events[0]["value"].as_object().unwrap().len(),
        4,
        "gmail.sent payload must contain only the published fields"
    );

    let repeat = run_gmail(&env, &fake, &["send", &note, "-f", "json"]);
    assert_success(&repeat, "repeat send");
    assert_eq!(json_stdout(&repeat)["status"], "already_sent");
    assert_eq!(
        calls_for(&fake, "send").len(),
        1,
        "never resend a completed operation"
    );
    let repeated_journal =
        fs::read_to_string(env.vault().join(".bob/gmail/test-host.jsonl"))
            .unwrap();
    assert_eq!(
        repeated_journal
            .lines()
            .filter(|line| line.contains("\"kind\":\"gmail.sent\""))
            .count(),
        1,
        "completed send journal replay must be idempotent: {repeated_journal}"
    );

    // The promoted note is an ordinary saved thread for list, save, pull,
    // and link: indexed at its original path and refreshed in place.
    let listed = run_gmail(&env, &fake, &["list", "-f", "json"]);
    assert_success(&listed, "list promoted note");
    let listed_doc = json_stdout(&listed);
    let listed_text = listed_doc.to_string();
    assert!(
        listed_text.contains(&note) && listed_text.contains("0123456789abcdef"),
        "promoted note must be indexed as a saved thread: {listed_doc}"
    );
    let refreshed =
        run_gmail(&env, &fake, &["save", "0123456789abcdef", "-f", "json"]);
    assert_success(&refreshed, "refresh promoted note through save");
    assert_eq!(json_stdout(&refreshed)["note"], note.as_str());
    let refreshed_note = fs::read_to_string(&note_path).unwrap();
    assert!(refreshed_note.contains("custom_owner: keep-me"));
    assert!(refreshed_note.contains("Private note."));
    assert!(refreshed_note.contains("> [!mail-sent"));
}

#[test]
fn reply_all_removes_only_the_sent_reply_after_confirmation() {
    let env = GmailEnv::new("bob-cli-gmail-send-reply-all");
    let fake = FakeGog::new(&env, "gog").scenario("compose_grant");
    let note = "mail/2026/lease.md";
    make_saved_thread(&env, &fake, note);
    let note_path = env.vault().join(note);
    let reply = run_gmail(&env, &fake, &["reply", note, "--all", "-f", "json"]);
    assert_success(&reply, "append reply all");
    fs::write(
        &note_path,
        format!(
            "{}Thanks, everyone.\n",
            fs::read_to_string(&note_path).unwrap()
        ),
    )
    .unwrap();

    let preview = run_gmail(&env, &fake, &["send", note, "-d", "-f", "json"]);
    assert_success(&preview, "preview reply-all send");
    let preview_doc = json_stdout(&preview);
    assert_eq!(preview_doc["preview"]["reply_target"]["mode"], "reply_all");
    assert_eq!(preview_doc["preview"]["to"][0], "Dana <dana@example.test>");
    assert_eq!(preview_doc["preview"]["cc"][0], "Lee <lee@example.test>");

    let sent =
        run_gmail_stdin(&env, &fake, &["send", note, "-f", "json"], "yes\n");
    assert_success(&sent, "TTY-confirmed reply-all send");
    assert!(stderr(&sent).contains("Send this exact Gmail draft? [y/N]"));
    assert_eq!(json_stdout(&sent)["status"], "sent");
    let finalized = fs::read_to_string(&note_path).unwrap();
    assert!(!finalized.contains("## Reply all"));
    assert!(!finalized.contains("Thanks, everyone."));
    assert!(!finalized.contains("bob:gmail:draft"));
    assert!(finalized.contains("Keep this private."));
    assert!(
        finalized.contains("> [!mail+] **Dana**"),
        "inbound card stays unchanged"
    );
    assert!(finalized.contains("> [!mail-sent+] **You**"));
    assert_eq!(calls_for(&fake, "send").len(), 1);
}

#[test]
fn tty_decline_keeps_the_draft_and_reply_section() {
    let env = GmailEnv::new("bob-cli-gmail-send-decline");
    let fake = FakeGog::new(&env, "gog").scenario("compose_grant");
    let note = "mail/2026/lease.md";
    make_saved_thread(&env, &fake, note);
    let note_path = env.vault().join(note);
    let reply = run_gmail(&env, &fake, &["reply", note, "-f", "json"]);
    assert_success(&reply, "append reply");
    fs::write(
        &note_path,
        format!("{}Maybe later.\n", fs::read_to_string(&note_path).unwrap()),
    )
    .unwrap();

    let output =
        run_gmail_stdin(&env, &fake, &["send", note, "-f", "json"], "no\n");
    assert_success(&output, "decline TTY prompt");
    assert_eq!(json_stdout(&output)["status"], "declined");
    assert!(stderr(&output).contains("Send this exact Gmail draft? [y/N]"));
    let kept = fs::read_to_string(note_path).unwrap();
    assert!(kept.contains("## Reply"));
    assert!(kept.contains("Maybe later."));
    assert!(kept.contains("bob:gmail:draft draft-1"));
    assert!(calls_for(&fake, "send").is_empty());
}

#[test]
fn html_only_and_remote_draft_edits_invalidate_confirmation() {
    let env = GmailEnv::new("bob-cli-gmail-send-edits");
    let fake = FakeGog::new(&env, "gog").scenario("compose_grant");
    let note = create_compose(&env, &fake, "Lease renewal");
    let note_path = env.vault().join(&note);
    let original = fs::read_to_string(&note_path).unwrap();
    fs::write(&note_path, format!("{original}Hello Ada.\n")).unwrap();

    let plain = run_gmail(&env, &fake, &["send", &note, "-d", "-f", "json"]);
    assert_success(&plain, "preview plain body");
    let plain_doc = json_stdout(&plain);
    let plain_digest = plain_doc["digest"].as_str().unwrap().to_string();

    // Bold changes only the HTML alternative; the plain text is identical.
    fs::write(&note_path, format!("{original}Hello **Ada**.\n")).unwrap();
    let stale = run_gmail(
        &env,
        &fake,
        &["send", &note, "-c", &plain_digest, "-f", "json"],
    );
    assert_eq!(stale.status.code(), Some(2), "{}", stderr(&stale));
    assert_eq!(
        json_stdout(&stale)["error"]["kind"],
        "confirmation_mismatch"
    );
    assert!(calls_for(&fake, "send").is_empty());

    let bold = run_gmail(&env, &fake, &["send", &note, "-d", "-f", "json"]);
    assert_success(&bold, "preview bold body");
    let bold_doc = json_stdout(&bold);
    assert_eq!(
        bold_doc["preview"]["text_body"], plain_doc["preview"]["text_body"],
        "the edit must be HTML-only"
    );
    assert_ne!(
        bold_doc["preview"]["html_sha256"],
        plain_doc["preview"]["html_sha256"]
    );
    let bold_digest = bold_doc["digest"].as_str().unwrap().to_string();
    assert_ne!(bold_digest, plain_digest);

    // An edit made in Gmail after the preview is a conflict, never silently
    // overwritten by the confirmation run's draft update.
    let updates_before = calls_for(&fake, "update").len();
    fake.edit_draft_header(
        bold_doc["draft_id"].as_str().unwrap(),
        "Subject",
        "Edited in Gmail",
    );
    let conflict = run_gmail(
        &env,
        &fake,
        &["send", &note, "-c", &bold_digest, "-f", "json"],
    );
    assert_ne!(conflict.status.code(), Some(0), "{}", stdout(&conflict));
    assert_eq!(
        json_stdout(&conflict)["error"]["kind"],
        "draft_remote_conflict"
    );
    assert_eq!(calls_for(&fake, "update").len(), updates_before);
    assert!(calls_for(&fake, "send").is_empty());
    assert!(fs::read_to_string(&note_path)
        .unwrap()
        .contains("status: draft"));
}

const WATCH_THREAD: &str = "0123456789abcdef";
const SIBLING_THREAD: &str = "fedcba9876543210";

fn write_watch_config(env: &GmailEnv) {
    env.write_config(&format!(
        "timezone: America/New_York\ngmail:\n  account: {ACCOUNT}\n  writer: {HOST}\n  sync:\n    reply_wait_days: 1\n"
    ));
}

fn write_blocked_task_settings(env: &GmailEnv) {
    let path = env
        .vault()
        .join(".obsidian/plugins/obsidian-tasks-plugin/data.json");
    fs::create_dir_all(path.parent().unwrap())
        .expect("create Tasks settings dir");
    fs::write(
        path,
        serde_json::to_vec(&json!({
            "globalFilter": "#task",
            "taskFormat": "dataview",
            "statusSettings": {
                "coreStatuses": [
                    {"symbol": " ", "name": "Ready", "type": "TODO"},
                    {"symbol": "x", "name": "Done", "type": "DONE"}
                ],
                "customStatuses": [
                    {"symbol": "*", "name": "Next", "nextStatusSymbol": "x", "availableAsCommand": true, "type": "ON_HOLD"},
                    {"symbol": "/", "name": "In Progress", "nextStatusSymbol": "x", "availableAsCommand": true, "type": "IN_PROGRESS"},
                    {"symbol": "?", "name": "Blocked", "nextStatusSymbol": " ", "availableAsCommand": true, "type": "ON_HOLD"},
                    {"symbol": "-", "name": "Canceled", "nextStatusSymbol": " ", "availableAsCommand": true, "type": "CANCELLED"}
                ]
            }
        }))
        .expect("serialize Tasks settings"),
    )
    .expect("write Tasks settings");
}

fn watch_message(
    thread_id: &str,
    id: &str,
    internal_date: i64,
    from: &str,
    to: &str,
    labels: &[&str],
) -> Value {
    json!({
        "id": id,
        "threadId": thread_id,
        "labelIds": labels,
        "internalDate": internal_date.to_string(),
        "payload": {
            "mimeType": "text/plain",
            "headers": [
                {"name": "From", "value": from},
                {"name": "To", "value": to},
                {"name": "Subject", "value": "Re: Watch contract"},
                {"name": "Message-ID", "value": format!("<{id}@example.test>")}
            ],
            "body": {"size": 5, "data": "SGVsbG8="}
        }
    })
}

fn inbound_thread(thread_id: &str, message_id: &str) -> Value {
    json!({
        "thread": {
            "id": thread_id,
            "historyId": "7",
            "messages": [watch_message(
                thread_id,
                message_id,
                1_793_786_400_000,
                "Dana <dana@example.test>",
                ACCOUNT,
                &["INBOX"],
            )]
        }
    })
}

fn enroll_thread(env: &GmailEnv, thread_id: &str, note: &str, task_file: &str) {
    let journal = env.vault().join(format!(".bob/gmail/{HOST}.jsonl"));
    fs::create_dir_all(journal.parent().unwrap()).expect("journal directory");
    let mut existing = fs::read_to_string(&journal).unwrap_or_default();
    existing.push_str(
        &json!({
            "v": 1,
            "ts": "2026-11-05T12:00:00-05:00",
            "host": HOST,
            "kind": "gmail.adopted",
            "value": {
                "account": ACCOUNT,
                "thread_id": thread_id,
                "note": note,
                "task_file": task_file
            }
        })
        .to_string(),
    );
    existing.push('\n');
    fs::write(journal, existing).expect("write enrollment");
}

fn journal_kinds(env: &GmailEnv, kind: &str) -> Vec<Value> {
    let path = env.vault().join(format!(".bob/gmail/{HOST}.jsonl"));
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|event| event["kind"] == kind)
        .collect()
}

fn first_task_line(path: &Path) -> String {
    fs::read_to_string(path)
        .expect("read task note")
        .lines()
        .find(|line| line.contains("#task") && line.contains("[[mail/"))
        .expect("mail task row")
        .to_string()
}

fn write_task_row(env: &GmailEnv, task_file: &str, line: &str) {
    let path = env.vault().join(task_file);
    fs::create_dir_all(path.parent().unwrap()).expect("task dir");
    fs::write(
        path,
        format!(
            "---\ntype: \"[[area]]\"\n---\n\n# Mail\n\n## Tasks\n\n{line}\n"
        ),
    )
    .expect("write task note");
}

fn save_thread(
    env: &GmailEnv,
    fake: &FakeGog,
    thread_id: &str,
) -> (String, String) {
    let saved = run_gmail(env, fake, &["save", thread_id, "-f", "json"]);
    assert_success(&saved, "save mail note");
    let doc = json_stdout(&saved);
    (
        doc["note"].as_str().expect("saved note path").to_string(),
        doc["link"].as_str().expect("saved note link").to_string(),
    )
}

fn send_confirmed_reply(env: &GmailEnv, fake: &FakeGog, note: &str) -> Value {
    let reply = run_gmail(env, fake, &["reply", note, "-f", "json"]);
    assert_success(&reply, "append reply");
    let note_path = env.vault().join(note);
    fs::write(
        &note_path,
        format!("{}Thanks.\n", fs::read_to_string(&note_path).unwrap()),
    )
    .unwrap();
    let preview = run_gmail(env, fake, &["send", note, "-d", "-f", "json"]);
    assert_success(&preview, "preview send");
    let digest = json_stdout(&preview)["digest"]
        .as_str()
        .unwrap()
        .to_string();
    let sent =
        run_gmail(env, fake, &["send", note, "-c", &digest, "-f", "json"]);
    assert_success(&sent, "confirmed send");
    json_stdout(&sent)
}

fn setup_watched_send(prefix: &str) -> (GmailEnv, FakeGog, String, String) {
    let env = GmailEnv::new(prefix);
    write_watch_config(&env);
    write_blocked_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("compose_grant");
    fake.set_thread(
        inbound_thread(WATCH_THREAD, "00000000000000a0"),
        json!({}),
    );
    let (note, link) = save_thread(&env, &fake, WATCH_THREAD);
    write_task_row(
        &env,
        "gmail_inbox.md",
        &format!("- [ ] #task Watch mail {link} [fresh:: 2026-11-04] ^watch1"),
    );
    enroll_thread(&env, WATCH_THREAD, &note, "gmail_inbox.md");
    (env, fake, note, link)
}

#[test]
fn confirmed_send_holds_an_enrolled_watched_task() {
    let (env, fake, note, _) =
        setup_watched_send("bob-cli-gmail-send-watch-hold");
    let sent = send_confirmed_reply(&env, &fake, &note);
    assert_eq!(sent["status"], "sent");
    let held = first_task_line(&env.vault().join("gmail_inbox.md"));
    assert!(held.starts_with("- [?] #task #awaiting_reply"), "{held}");
    assert!(held.contains("[scheduled::"), "{held}");
    let turns = journal_kinds(&env, "gmail.turn");
    assert_eq!(turns.len(), 1, "{turns:?}");
    assert_eq!(turns[0]["value"]["thread_id"], WATCH_THREAD);
    assert_eq!(turns[0]["value"]["effect"], "hold");
    assert_eq!(calls_for(&fake, "send").len(), 1);

    let poll = run_gmail(&env, &fake, &["sync", "-l", "0", "-f", "json"]);
    assert_success(&poll, "subsequent poll after send hold");
    let poll_doc = json_stdout(&poll);
    assert!(
        poll_doc["watch"]
            .as_array()
            .into_iter()
            .flatten()
            .all(|row| row["action"] == "unchanged"),
        "send-consumed turn must not reapply on poll: {poll_doc}"
    );
    assert_eq!(
        first_task_line(&env.vault().join("gmail_inbox.md")),
        held,
        "poll must not rewrite the send hold"
    );
}

#[test]
fn confirmed_send_holds_every_watcher_of_the_thread() {
    let (env, fake, note, link) =
        setup_watched_send("bob-cli-gmail-send-watch-many");
    write_task_row(
        &env,
        "job.md",
        &format!(
            "- [ ] #task Second watcher {link} [fresh:: 2026-11-04] ^watch2"
        ),
    );
    enroll_thread(&env, WATCH_THREAD, &note, "job.md");
    send_confirmed_reply(&env, &fake, &note);
    for path in ["gmail_inbox.md", "job.md"] {
        let line = first_task_line(&env.vault().join(path));
        assert!(
            line.contains("#awaiting_reply"),
            "{path} was not held: {line}"
        );
    }
}

#[test]
fn unwatched_send_does_not_hold_or_mark_a_turn() {
    let env = GmailEnv::new("bob-cli-gmail-send-unwatched");
    write_watch_config(&env);
    write_blocked_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("compose_grant");
    let note = "mail/2026/lease.md";
    make_saved_thread(&env, &fake, note);
    send_confirmed_reply(&env, &fake, note);
    assert!(journal_kinds(&env, "gmail.turn").is_empty());
    assert!(!env.vault().join("gmail_inbox.md").exists());
}

#[test]
fn send_defers_a_running_hold_and_lets_sync_finish_it() {
    let (env, fake, note, _) =
        setup_watched_send("bob-cli-gmail-send-watch-running");
    let daily = env.vault().join("2026/20261105.md");
    fs::create_dir_all(daily.parent().unwrap()).expect("daily dir");
    fs::write(
        &daily,
        "# Thursday 2026-11-05\n\n## Pomodoros\n- [ ] (**1200-1230** [t:: 30m]) Mail\n  - [[gmail_inbox#^watch1]]\n",
    )
    .expect("write running ledger");
    send_confirmed_reply(&env, &fake, &note);
    let line = first_task_line(&env.vault().join("gmail_inbox.md"));
    assert!(
        !line.contains("#awaiting_reply"),
        "running session must defer the hold: {line}"
    );
    assert!(
        journal_kinds(&env, "gmail.turn").is_empty(),
        "deferred running hold must not mark the turn applied"
    );
    assert_eq!(calls_for(&fake, "send").len(), 1);

    fs::write(&daily, "# Thursday 2026-11-05\n\n## Pomodoros\n")
        .expect("close the running session");
    let held = run_gmail(&env, &fake, &["sync", "-l", "0", "-f", "json"]);
    assert_success(&held, "sync finishes the deferred hold");
    let held_line = first_task_line(&env.vault().join("gmail_inbox.md"));
    assert!(
        held_line.contains("#awaiting_reply"),
        "next sync must hold after the session closes: {held_line}"
    );
    assert_eq!(journal_kinds(&env, "gmail.turn").len(), 1);
    assert_eq!(calls_for(&fake, "send").len(), 1);
}

#[test]
fn send_does_not_hold_when_sibling_metadata_is_unavailable() {
    let (env, fake, note, link) =
        setup_watched_send("bob-cli-gmail-send-watch-sibling-fail");
    fake.set_threads(json!({
        SIBLING_THREAD: inbound_thread(SIBLING_THREAD, "00000000000000b0")
    }));
    let (sibling_note, sibling_link) = save_thread(&env, &fake, SIBLING_THREAD);
    write_task_row(
        &env,
        "gmail_inbox.md",
        &format!(
            "- [ ] #task Watch mail {link} {sibling_link} [fresh:: 2026-11-04] ^watch1"
        ),
    );
    enroll_thread(&env, SIBLING_THREAD, &sibling_note, "gmail_inbox.md");
    fake.set_thread_fail(SIBLING_THREAD, "notFound: sibling missing", 1);
    send_confirmed_reply(&env, &fake, &note);
    let line = first_task_line(&env.vault().join("gmail_inbox.md"));
    assert!(
        !line.contains("#awaiting_reply"),
        "unknown sibling must not claim every thread is waiting: {line}"
    );
    assert!(journal_kinds(&env, "gmail.turn").is_empty());
    let finalized = fs::read_to_string(env.vault().join(&note)).unwrap();
    assert!(finalized.contains("> [!mail-sent+] **You**"));
}

#[test]
fn send_lifts_when_a_sibling_incoming_move_has_precedence() {
    let (env, fake, note, link) =
        setup_watched_send("bob-cli-gmail-send-watch-lift");
    fake.set_threads(json!({
        SIBLING_THREAD: inbound_thread(SIBLING_THREAD, "00000000000000b0")
    }));
    let (sibling_note, sibling_link) = save_thread(&env, &fake, SIBLING_THREAD);
    write_task_row(
        &env,
        "gmail_inbox.md",
        &format!(
            "- [?] #task #awaiting_reply Watch mail {link} {sibling_link} [scheduled:: 2026-11-06] [fresh:: 2026-11-04] ^watch1"
        ),
    );
    enroll_thread(&env, SIBLING_THREAD, &sibling_note, "gmail_inbox.md");
    send_confirmed_reply(&env, &fake, &note);
    let line = first_task_line(&env.vault().join("gmail_inbox.md"));
    assert!(
        !line.contains("#awaiting_reply"),
        "incoming sibling must lift rather than hold: {line}"
    );
    assert!(!line.contains("[scheduled::"), "{line}");
}

#[test]
fn complete_send_retries_watch_marker_without_a_second_gmail_send() {
    let (env, fake, note, _) =
        setup_watched_send("bob-cli-gmail-send-watch-retry");
    send_confirmed_reply(&env, &fake, &note);
    assert_eq!(calls_for(&fake, "send").len(), 1);
    assert_eq!(journal_kinds(&env, "gmail.turn").len(), 1);

    let journal_path = env.vault().join(format!(".bob/gmail/{HOST}.jsonl"));
    let kept: String = fs::read_to_string(&journal_path)
        .unwrap()
        .lines()
        .filter(|line| !line.contains("\"kind\":\"gmail.turn\""))
        .map(|line| format!("{line}\n"))
        .collect();
    fs::write(&journal_path, kept).expect("drop the watch marker");
    let inbox = env.vault().join("gmail_inbox.md");
    let held = first_task_line(&inbox);
    let edited =
        held.replace("[scheduled:: 2026-11-06]", "[scheduled:: 2026-11-09]");
    let edited = if edited == held {
        held.replace(
            first_scheduled(&held).as_str(),
            "[scheduled:: 2026-11-09]",
        )
    } else {
        edited
    };
    fs::write(
        &inbox,
        fs::read_to_string(&inbox).unwrap().replace(&held, &edited),
    )
    .expect("edit the held task after the partial completion");

    let repeat = run_gmail(&env, &fake, &["send", &note, "-f", "json"]);
    assert_success(&repeat, "retry completed send");
    assert_eq!(json_stdout(&repeat)["status"], "already_sent");
    assert_eq!(calls_for(&fake, "send").len(), 1);
    assert_eq!(journal_kinds(&env, "gmail.turn").len(), 1);
    let retried = first_task_line(&inbox);
    assert!(
        retried.contains("[scheduled:: 2026-11-09]"),
        "retry must preserve the later task edit: {retried}"
    );
    assert_eq!(retried.matches("#awaiting_reply").count(), 1, "{retried}");
}

fn first_scheduled(line: &str) -> String {
    let start = line.find("[scheduled:: ").expect("scheduled field");
    let rest = &line[start..];
    let end = rest.find(']').expect("scheduled close");
    rest[..=end].to_string()
}
