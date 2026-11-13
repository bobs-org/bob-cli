//! `bob gmail sync` live archive apply: one-shot mutation, let-go, and
//! crash reconciliation over the fake transport.
//!
//! Every live run must keep durable filings when archive setup or per-thread
//! work fails, isolate non-auth failures, abort on `invalid_grant`, create
//! no labels for paused batches, and converge crashes without reclaiming.
//!
//! Core conformance audit (C1-C20):
//! C1 `live_archives_a_fresh_close_and_repeats_as_noop`; C2
//! `cancelled_linker_is_positive_archive_evidence`; C3/C4/C5
//! `archive_scan::{open_linkers_veto_including_open_rows_in_done,
//! no_linkers_means_no_evidence,positive_closed_covers_cancelled_uppercase_and_done}`;
//! C6/C7 `archive_preview::{remote_verdict_keeps_unsaved_consent_and_policy,
//! remote_verdict_holds_unresolvable_and_manual_or_reconcile}`; C8
//! `live_keeps_unsaved_inbox_mail_without_mutation`; C9/C10
//! `new_reply_lets_go_and_preserves_inbox` / `manual_move_to_inbox_lets_go_without_removing_inbox`;
//! C11 `reopening_an_archived_task_reports_a_restore_hint_without_writing`;
//! C12/C13 `archive_preview::gate_requires_backlog_and_burst_confirmation` /
//! `gmail_archive_preview::dry_run_pauses_backlog_and_confirms_with_the_digest`;
//! C14 `live_reconciles_after_remote_success_and_journal_failure_without_remutating`;
//! C15 `archive_preview::remote_verdict_keeps_unsaved_consent_and_policy`;
//! C16 `live_missing_modify_grant_is_setup_with_login_hint` plus the dry-run
//! cases in `gmail_archive_preview`; C17/C18
//! `archive_preview::{local_verdict_holds_merge_and_identity_evidence,
//! local_verdict_suppresses_same_run_incoming_moves}`; C19
//! `gmail_archive_preview::dry_run_pauses_backlog_and_confirms_with_the_digest`
//! and `cli::sync_accepts_confirm_no_archive_and_preserves_pull`; C20
//! `archive_reports_and_journals_never_echo_hostile_task_text` and the
//! body-free terminal tests in `archive_state`.

mod gmail_support;

use std::fs;

use gmail_support::{
    json_stdout, run_gmail, stderr, stdout, FakeGog, GmailEnv, ACCOUNT, HOST,
};
use serde_json::{json, Value};

const THREAD_A: &str = "0123456789abcdef";
const THREAD_B: &str = "aaaaaaaaaaaaaaaa";
const MESSAGE_A: &str = "00000000000000a0";
const MESSAGE_B: &str = "00000000000000b1";
const ENROLLMENT_ID: &str = "Label_9";
const RECEIPT_ID: &str = "Label_10";

fn assert_exit(output: &std::process::Output, expected: i32, label: &str) {
    assert_eq!(
        output.status.code(),
        Some(expected),
        "{label} expected exit {expected}:\nstdout:\n{}\nstderr:\n{}",
        stdout(output),
        stderr(output)
    );
}

fn write_archive_config(env: &GmailEnv) {
    env.write_config(&format!(
        "timezone: America/New_York\ngmail:\n  account: {ACCOUNT}\n  writer: {HOST}\n  sync:\n    archive: on\n"
    ));
}

fn write_task_settings(env: &GmailEnv) {
    let path = env
        .vault()
        .join(".obsidian/plugins/obsidian-tasks-plugin/data.json");
    fs::create_dir_all(path.parent().unwrap()).expect("tasks dir");
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
        .expect("serialize settings"),
    )
    .expect("write settings");
}

fn seed_labels(fake: &FakeGog, with_receipt: bool) {
    let mut labels = vec![
        json!({"id": ENROLLMENT_ID, "name": "bob", "type": "user"}),
        json!({"id": "INBOX", "name": "INBOX", "type": "system"}),
    ];
    if with_receipt {
        labels.push(
            json!({"id": RECEIPT_ID, "name": "bob/closed", "type": "user"}),
        );
    }
    fake.set_labels(Value::Array(labels));
}

fn thread_message(id: &str, labels: &[&str]) -> Value {
    json!({
        "id": id,
        "threadId": THREAD_A,
        "labelIds": labels,
        "internalDate": "1793908800000",
        "snippet": "A metadata-only archive fixture.",
        "payload": {
            "mimeType": "text/plain",
            "headers": [
                {"name": "From", "value": "Ada <ada@example.com>"},
                {"name": "To", "value": ACCOUNT},
                {"name": "Subject", "value": "Archive apply probe"},
                {"name": "Message-ID", "value": format!("<{id}@example.test>")}
            ],
            "body": {"size": 5, "data": "SGVsbG8="}
        }
    })
}

fn seed_thread_and_messages(fake: &FakeGog, messages: Vec<Value>) {
    fake.set_threads(json!({
        THREAD_A: {"thread": {"id": THREAD_A, "historyId": "1", "messages": messages.clone()}}
    }));
    let mut map = serde_json::Map::new();
    for message in &messages {
        let id = message["id"].as_str().expect("message id").to_string();
        let label_ids = message["labelIds"].clone();
        map.insert(id, json!({"threadId": THREAD_A, "labelIds": label_ids}));
    }
    fake.set_messages(Value::Object(map));
}

fn journal_text(env: &GmailEnv) -> String {
    let dir = env.vault().join(".bob/gmail");
    let Ok(entries) = fs::read_dir(dir) else {
        return String::new();
    };
    let mut lines: Vec<String> = entries
        .flatten()
        .filter_map(|entry| fs::read_to_string(entry.path()).ok())
        .collect();
    lines.sort();
    lines.join("\n")
}

fn journal_has(env: &GmailEnv, kind: &str) -> bool {
    journal_text(env).contains(kind)
}

fn remote_write_calls(fake: &FakeGog) -> Vec<Vec<String>> {
    fake.calls()
        .into_iter()
        .filter(|call| {
            call.iter().any(|arg| {
                arg == "gmail.batch.modify" || arg == "gmail.labels.create"
            })
        })
        .collect()
}

fn file_adopt(env: &GmailEnv, fake: &FakeGog) -> (String, String) {
    let saved = run_gmail(env, fake, &["save", THREAD_A, "-f", "json"]);
    assert_eq!(
        saved.status.code(),
        Some(0),
        "save the watched mail note:\n{}\n{}",
        stdout(&saved),
        stderr(&saved)
    );
    let doc = json_stdout(&saved);
    let note = doc["note"].as_str().expect("saved note").to_string();
    let link = doc["link"].as_str().expect("saved link").to_string();
    let inbox = env.vault().join("gmail_inbox.md");
    fs::write(
        &inbox,
        format!(
            "#task archive apply probe {link}\n\n- [ ] #task archive apply probe {link}\n"
        ),
    )
    .expect("write inbox task");
    let adopted =
        run_gmail(env, fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_eq!(
        adopted.status.code(),
        Some(0),
        "adopt the watched thread:\n{}\n{}",
        stdout(&adopted),
        stderr(&adopted)
    );
    (note, link)
}

fn close_inbox_task(env: &GmailEnv) {
    set_inbox_task_status(env, 'x');
}

fn set_inbox_task_status(env: &GmailEnv, status: char) {
    let inbox = env.vault().join("gmail_inbox.md");
    let text = fs::read_to_string(&inbox).expect("read inbox");
    let task = text
        .lines()
        .find(|line| line.starts_with("- [") && line.contains("#task"))
        .expect("inbox task row");
    let replacement = format!("- [{status}]{}", &task[5..]);
    fs::write(&inbox, text.replacen(task, &replacement, 1))
        .expect("update inbox task status");
}

#[test]
fn live_archives_a_fresh_close_and_repeats_as_noop() {
    let env = GmailEnv::new("bob-cli-gmail-archive-apply-fresh");
    env.write_journal_rules();
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    let (note, _) = file_adopt(&env, &fake);
    fake.clear_calls();
    close_inbox_task(&env);
    let archived =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&archived, 0, "fresh close archives without confirm");
    let value = json_stdout(&archived);
    let archive = value["archive"].as_array().expect("archive rows");
    assert_eq!(archive.len(), 1, "one live archive row");
    assert_eq!(archive[0]["thread_id"], THREAD_A);
    assert_eq!(archive[0]["action"], "archived");
    assert_eq!(archive[0]["messages"], json!([MESSAGE_A]));
    assert!(
        journal_has(&env, "gmail.archived"),
        "journal records the archive"
    );
    assert_eq!(
        fake.message_labels(MESSAGE_A),
        vec![RECEIPT_ID.to_string(), ENROLLMENT_ID.to_string()],
        "INBOX removed and receipt added"
    );
    let writes = remote_write_calls(&fake);
    assert!(
        writes
            .iter()
            .any(|call| call.contains(&"gmail.batch.modify".to_string())),
        "one batch modify ran"
    );
    assert!(
        !writes
            .iter()
            .any(|call| call.contains(&"gmail.labels.create".to_string())),
        "existing receipt needs no label creation"
    );
    let doctor = run_gmail(&env, &fake, &["doctor", "-f", "json"]);
    assert_exit(&doctor, 0, "doctor after archive");
    let doctor_json = json_stdout(&doctor);
    let archive_check = doctor_json["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["id"] == "archive")
        .expect("archive doctor row");
    assert!(archive_check["summary"]
        .as_str()
        .unwrap()
        .contains("this week 1 archived · 0 kept"));
    fake.clear_calls();
    let again =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&again, 0, "repeat sync is a no-op");
    assert!(
        json_stdout(&again)["archive"]
            .as_array()
            .expect("archive")
            .is_empty(),
        "consumed close never re-archives"
    );
    assert!(remote_write_calls(&fake).is_empty(), "no-op writes nothing");
    assert!(
        env.vault().join(&note).is_file(),
        "filing commits precede mutation and remain"
    );
}

#[test]
fn first_use_receipt_id_is_journaled_and_released_by_id() {
    let env = GmailEnv::new("bob-cli-gmail-archive-first-use-label");
    env.write_journal_rules();
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, false);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    file_adopt(&env, &fake);
    close_inbox_task(&env);

    let archived =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&archived, 0, "first-use archive creates its receipt label");
    assert_eq!(json_stdout(&archived)["archive"][0]["action"], "archived");
    assert_eq!(
        fake.message_labels(MESSAGE_A),
        vec!["Label_1", ENROLLMENT_ID]
    );
    let journal = journal_text(&env);
    assert!(journal.contains("gmail.archived"));
    assert!(journal.contains("\"id\":\"Label_1\""));

    seed_thread_and_messages(
        &fake,
        vec![
            thread_message(MESSAGE_A, &["Label_1", ENROLLMENT_ID]),
            thread_message(MESSAGE_B, &["INBOX"]),
        ],
    );
    fake.clear_calls();
    let returned =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&returned, 0, "reply lets go of the created receipt by ID");
    assert_eq!(json_stdout(&returned)["archive"][0]["action"], "let_go");
    assert_eq!(fake.message_labels(MESSAGE_A), vec![ENROLLMENT_ID]);
    assert_eq!(fake.message_labels(MESSAGE_B), vec!["INBOX"]);
    assert!(journal_text(&env).contains("gmail.let_go"));
}

#[test]
fn archive_label_rename_rebaselines_and_keeps_old_receipt_identity() {
    let env = GmailEnv::new("bob-cli-gmail-archive-label-rename");
    env.write_journal_rules();
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    file_adopt(&env, &fake);
    close_inbox_task(&env);
    let archived =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&archived, 0, "archive under the original receipt name");

    set_inbox_task_status(&env, ' ');
    seed_thread_and_messages(
        &fake,
        vec![
            thread_message(MESSAGE_A, &[RECEIPT_ID, ENROLLMENT_ID]),
            thread_message(MESSAGE_B, &["INBOX"]),
        ],
    );
    env.write_config(&format!(
        "timezone: America/New_York\ngmail:\n  account: {ACCOUNT}\n  writer: {HOST}\n  sync:\n    archive: on\n    archive_label: bob/finished\n"
    ));
    fake.set_labels(json!([
        {"id": ENROLLMENT_ID, "name": "bob", "type": "user"},
        {"id": "INBOX", "name": "INBOX", "type": "system"},
        {"id": RECEIPT_ID, "name": "bob/closed", "type": "user"},
        {"id": "Label_11", "name": "bob/finished", "type": "user"}
    ]));

    let released =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(
        &released,
        0,
        "renamed label rebaseline still releases old receipt",
    );
    let value = json_stdout(&released);
    assert_eq!(value["archive"][0]["action"], "let_go");
    assert_eq!(value["archive"][0]["reason"], "replied");
    assert_eq!(fake.message_labels(MESSAGE_A), vec![ENROLLMENT_ID]);
    assert_eq!(fake.message_labels(MESSAGE_B), vec!["INBOX"]);
    let journal = journal_text(&env);
    assert!(journal.contains("bob/finished"));
    assert!(journal.contains("Label_11"));
    assert!(journal.contains("gmail.let_go"));
}

#[test]
fn saved_messages_added_after_close_do_not_widen_the_retry_set() {
    let env = GmailEnv::new("bob-cli-gmail-archive-frozen-saved-ids");
    env.write_journal_rules();
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    file_adopt(&env, &fake);
    close_inbox_task(&env);
    fake.set_transient_failure(
        "batch_modify",
        "temporary archive failure",
        1,
        1,
    );
    let first =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&first, 1, "first archive attempt fails transiently");
    assert!(journal_has(&env, "gmail.archive_closed"));
    assert!(!journal_has(&env, "gmail.archived"));

    seed_thread_and_messages(
        &fake,
        vec![
            thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID]),
            thread_message(MESSAGE_B, &["INBOX", ENROLLMENT_ID]),
        ],
    );
    let saved = run_gmail(&env, &fake, &["save", THREAD_A, "-f", "json"]);
    assert_exit(&saved, 0, "save the new message after the close");
    fake.clear_calls();

    let retry =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(
        &retry,
        0,
        "retry keeps the newly saved ID outside the close",
    );
    let value = json_stdout(&retry);
    assert_eq!(value["archive"][0]["action"], "kept");
    assert_eq!(value["archive"][0]["reason"], "unsaved");
    assert!(remote_write_calls(&fake).is_empty());
    assert!(fake
        .message_labels(MESSAGE_A)
        .contains(&"INBOX".to_string()));
    assert!(fake
        .message_labels(MESSAGE_B)
        .contains(&"INBOX".to_string()));
    assert!(journal_has(&env, "gmail.kept"));
    assert!(!journal_has(&env, "gmail.archived"));
}

#[test]
fn question_mark_linker_can_be_observed_open_and_archived_after_closing() {
    let env = GmailEnv::new("bob-cli-gmail-archive-question-status");
    env.write_journal_rules();
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    let saved = run_gmail(&env, &fake, &["save", THREAD_A, "-f", "json"]);
    assert_exit(&saved, 0, "save the mail note");
    let saved_json = json_stdout(&saved);
    let link = saved_json["link"].as_str().unwrap();
    fs::write(
        env.vault().join("gmail_inbox.md"),
        format!("#task question status probe {link}\n\n- [?] #task question status probe {link}\n"),
    )
    .expect("write question-mark task");

    let observed =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&observed, 0, "observe the question-mark task as open");
    assert!(json_stdout(&observed)["archive"]
        .as_array()
        .unwrap()
        .is_empty());
    set_inbox_task_status(&env, 'x');
    let archived =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&archived, 0, "archive after the question-mark task closes");
    assert_eq!(json_stdout(&archived)["archive"][0]["action"], "archived");
}

#[test]
fn live_pauses_backlog_without_confirm_and_creates_no_labels() {
    let env = GmailEnv::new("bob-cli-gmail-archive-apply-backlog");
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, false);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    let journal_dir = env.vault().join(".bob/gmail");
    fs::create_dir_all(&journal_dir).expect("journal dir");
    let label = json!({"name": "bob/closed", "id": Value::Null});
    let epoch = {
        use sha2::{Digest, Sha256};
        format!(
            "sha256:{}",
            hex::encode(Sha256::digest(
                serde_json::to_vec(&json!({
                    "purpose": "gmail.archive.epoch",
                    "account": ACCOUNT,
                    "previous_change_id": Value::Null,
                    "label": label,
                }))
                .unwrap()
            ))
        )
    };
    let change = {
        use sha2::{Digest, Sha256};
        format!(
            "sha256:{}",
            hex::encode(Sha256::digest(
                serde_json::to_vec(&json!({
                    "purpose": "gmail.archive.enablement",
                    "account": ACCOUNT,
                    "enabled": true,
                    "epoch": epoch,
                    "previous_change_id": Value::Null,
                    "label": label,
                }))
                .unwrap()
            ))
        )
    };
    fs::write(
        journal_dir.join("athena.jsonl"),
        serde_json::to_string(&json!({
            "v": 1,
            "ts": "2026-01-05T09:00:00-05:00",
            "host": "athena",
            "kind": "gmail.archive_enabled",
            "value": {
                "account": ACCOUNT,
                "enabled": true,
                "epoch": epoch,
                "change_id": change,
                "previous_change_id": Value::Null,
                "label": label,
            },
        }))
        .unwrap()
            + "\n",
    )
    .expect("seed enablement");
    // Adopt without archive observations so the first observed close is a
    // historical backlog: the initial live sync skips archive with `-n`.
    let saved = run_gmail(&env, &fake, &["save", THREAD_A, "-f", "json"]);
    assert_eq!(
        saved.status.code(),
        Some(0),
        "save the watched mail note:\n{}\n{}",
        stdout(&saved),
        stderr(&saved)
    );
    let link = json_stdout(&saved)["link"]
        .as_str()
        .expect("saved link")
        .to_string();
    let inbox = env.vault().join("gmail_inbox.md");
    fs::write(
        &inbox,
        format!("#task archive apply probe {link}\n\n- [ ] #task archive apply probe {link}\n"),
    )
    .expect("write inbox task");
    let adopted = run_gmail(
        &env,
        &fake,
        &["sync", "-l", "0", "-r", "0", "-n", "-f", "json"],
    );
    assert_eq!(
        adopted.status.code(),
        Some(0),
        "adopt without archive observations:\n{}\n{}",
        stdout(&adopted),
        stderr(&adopted)
    );
    close_inbox_task(&env);
    fake.clear_calls();
    let paused =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&paused, 1, "unconfirmed backlog pauses");
    let value = json_stdout(&paused);
    assert_eq!(value["archive"][0]["action"], "paused");
    assert_eq!(value["archive"][0]["reason"], "backlog_unconfirmed");
    let digest = value["archive_digest"]
        .as_str()
        .expect("digest")
        .to_string();
    assert!(
        remote_write_calls(&fake).is_empty(),
        "paused creates no labels"
    );
    assert!(
        !journal_has(&env, "gmail.archived"),
        "paused appends no terminal"
    );
    let confirmed = run_gmail(
        &env,
        &fake,
        &["sync", "-l", "0", "-r", "0", "-c", &digest, "-f", "json"],
    );
    assert_exit(&confirmed, 0, "matching digest confirms the backlog");
    assert_eq!(json_stdout(&confirmed)["archive"][0]["action"], "archived");
    assert!(
        journal_has(&env, "gmail.archived"),
        "confirmed backlog archives"
    );
}

#[test]
fn live_keeps_unsaved_inbox_mail_without_mutation() {
    let env = GmailEnv::new("bob-cli-gmail-archive-apply-unsaved");
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    let (_note, _) = file_adopt(&env, &fake);
    close_inbox_task(&env);
    seed_thread_and_messages(
        &fake,
        vec![
            thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID]),
            thread_message(MESSAGE_B, &["INBOX", ENROLLMENT_ID]),
        ],
    );
    fake.set_messages(json!({
        MESSAGE_A: {"threadId": THREAD_A, "labelIds": ["INBOX", ENROLLMENT_ID]},
        MESSAGE_B: {"threadId": THREAD_A, "labelIds": ["INBOX", ENROLLMENT_ID]},
    }));
    fake.clear_calls();
    let kept =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&kept, 0, "unsaved mail keeps the thread");
    let value = json_stdout(&kept);
    assert_eq!(value["archive"][0]["action"], "kept");
    assert_eq!(value["archive"][0]["reason"], "unsaved");
    assert!(remote_write_calls(&fake).is_empty(), "kept writes nothing");
    assert!(journal_has(&env, "gmail.kept"), "kept consumes the close");
    assert_eq!(
        fake.message_labels(MESSAGE_A),
        vec!["INBOX".to_string(), ENROLLMENT_ID.to_string()],
        "kept leaves Gmail untouched"
    );
}

#[test]
fn live_isolates_one_thread_failure_and_keeps_filings() {
    let env = GmailEnv::new("bob-cli-gmail-archive-apply-isolation");
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    let (_note, _) = file_adopt(&env, &fake);
    close_inbox_task(&env);
    fake.set_failure("batch_modify", "transient: PRIVATE_SENTINEL flap", 1);
    let failed =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&failed, 1, "isolated failure fails the run");
    assert!(
        !journal_has(&env, "gmail.archived"),
        "failure consumes nothing"
    );
    assert!(
        fs::read_to_string(env.vault().join("gmail_inbox.md")).is_ok(),
        "successful filings remain after failure"
    );
}

#[test]
fn live_missing_modify_grant_is_setup_with_login_hint() {
    let env = GmailEnv::new("bob-cli-gmail-archive-apply-grant");
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    let (_note, _) = file_adopt(&env, &fake);
    close_inbox_task(&env);
    fake.clear_calls();
    let denied =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&denied, 2, "missing modify grant is setup");
    let denied_json = json_stdout(&denied);
    assert_eq!(denied_json["error"]["kind"], "modify_grant");
    assert!(
        denied_json["error"]["hint"]
            .as_str()
            .unwrap_or_default()
            .contains("bob gmail login -s modify"),
        "login hint is loud:\n{denied_json}"
    );
    assert!(
        fs::read_to_string(env.vault().join("gmail_inbox.md")).is_ok(),
        "filings remain after setup failure"
    );
}

#[test]
fn live_reconciles_after_remote_success_and_journal_failure_without_remutating()
{
    let env = GmailEnv::new("bob-cli-gmail-archive-apply-recover");
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    let (_note, _) = file_adopt(&env, &fake);
    close_inbox_task(&env);
    fake.make_archive_journal_readonly_after_batch();
    let interrupted =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&interrupted, 1, "journal append fails after remote success");
    assert!(
        !journal_has(&env, "gmail.archived"),
        "the failed local append did not claim ownership"
    );
    assert_eq!(
        fake.message_labels(MESSAGE_A),
        vec![RECEIPT_ID.to_string(), ENROLLMENT_ID.to_string()],
        "the remote archive succeeded before the journal failure"
    );
    let journal = env.vault().join(".bob/gmail/test-host.jsonl");
    let mut permissions = fs::metadata(&journal)
        .expect("journal metadata")
        .permissions();
    permissions.set_readonly(false);
    fs::set_permissions(&journal, permissions).expect("restore journal writes");
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &[RECEIPT_ID, ENROLLMENT_ID])],
    );
    fake.clear_calls();
    let again =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&again, 0, "receipt evidence reconciles the archive");
    let value = json_stdout(&again);
    assert_eq!(value["archive"][0]["action"], "archived", "{value}");
    assert_eq!(value["archive"][0]["reason"], "recovered");
    assert!(journal_text(&env).contains("\"recovered\":true"));
    assert!(
        remote_write_calls(&fake).is_empty(),
        "recovery does not mutate"
    );
    let _ = THREAD_B;
}

#[test]
fn cancelled_linker_is_positive_archive_evidence() {
    let env = GmailEnv::new("bob-cli-gmail-archive-cancelled");
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    file_adopt(&env, &fake);
    set_inbox_task_status(&env, '-');
    let archived =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&archived, 0, "cancelled task is a positive close");
    assert_eq!(json_stdout(&archived)["archive"][0]["action"], "archived");
    assert_eq!(
        fake.message_labels(MESSAGE_A),
        vec![RECEIPT_ID.to_string(), ENROLLMENT_ID.to_string()]
    );
}

#[test]
fn new_reply_lets_go_and_preserves_inbox() {
    let env = GmailEnv::new("bob-cli-gmail-archive-reply-let-go");
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    file_adopt(&env, &fake);
    close_inbox_task(&env);
    let archived =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&archived, 0, "archive the closed thread");
    seed_thread_and_messages(
        &fake,
        vec![
            thread_message(MESSAGE_A, &[RECEIPT_ID, ENROLLMENT_ID]),
            thread_message(MESSAGE_B, &["INBOX"]),
        ],
    );
    fake.clear_calls();
    let returned =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&returned, 0, "new reply releases Bob's receipt");
    let value = json_stdout(&returned);
    assert_eq!(value["archive"][0]["action"], "let_go");
    assert_eq!(value["archive"][0]["reason"], "replied");
    assert_eq!(fake.message_labels(MESSAGE_A), vec![ENROLLMENT_ID]);
    assert_eq!(fake.message_labels(MESSAGE_B), vec!["INBOX"]);
    assert!(journal_has(&env, "gmail.let_go"));
    fake.clear_calls();
    let again =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&again, 0, "the released close stays consumed");
    assert!(remote_write_calls(&fake).is_empty());
}

#[test]
fn manual_move_to_inbox_lets_go_without_removing_inbox() {
    let env = GmailEnv::new("bob-cli-gmail-archive-manual-let-go");
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    file_adopt(&env, &fake);
    close_inbox_task(&env);
    let archived =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&archived, 0, "archive the closed thread");
    seed_thread_and_messages(
        &fake,
        vec![thread_message(
            MESSAGE_A,
            &["INBOX", RECEIPT_ID, ENROLLMENT_ID],
        )],
    );
    fake.clear_calls();
    let returned =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&returned, 0, "manual return releases Bob's receipt");
    let value = json_stdout(&returned);
    assert_eq!(value["archive"][0]["action"], "let_go");
    assert_eq!(value["archive"][0]["reason"], "manual_return");
    assert_eq!(
        fake.message_labels(MESSAGE_A),
        vec!["INBOX".to_string(), ENROLLMENT_ID.to_string()]
    );
    assert!(journal_has(&env, "gmail.let_go"));
}

#[test]
fn reopening_an_archived_task_reports_a_restore_hint_without_writing() {
    let env = GmailEnv::new("bob-cli-gmail-archive-reopen-hint");
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    file_adopt(&env, &fake);
    close_inbox_task(&env);
    let archived =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&archived, 0, "archive the closed thread");
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &[RECEIPT_ID, ENROLLMENT_ID])],
    );
    set_inbox_task_status(&env, ' ');
    fake.clear_calls();
    let reopened =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&reopened, 0, "reopened task receives a hint");
    let value = json_stdout(&reopened);
    assert_eq!(value["archive_hints"][0]["kind"], "restore_to_inbox");
    assert_eq!(value["archive_hints"][0]["thread_id"], THREAD_A);
    assert!(value["archive_hints"][0]["message"]
        .as_str()
        .unwrap()
        .contains("Move it to Inbox"));
    assert!(
        remote_write_calls(&fake).is_empty(),
        "reopen does not mutate Gmail"
    );
    let human = run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0"]);
    assert_exit(&human, 0, "human reopen hint");
    assert!(stdout(&human).contains("move it to Inbox"));
    assert!(remote_write_calls(&fake).is_empty());
}

#[test]
fn archive_reports_and_journals_never_echo_hostile_task_text() {
    let env = GmailEnv::new("bob-cli-gmail-archive-hostile-text");
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    file_adopt(&env, &fake);
    let inbox = env.vault().join("gmail_inbox.md");
    let text = fs::read_to_string(&inbox).expect("read task note");
    let hostile = "PRIVATE TASK TEXT \u{1b}[31m\u{1b}[2J";
    let replacement = format!("- [ ] #task {hostile}");
    fs::write(
        &inbox,
        text.replacen("- [ ] #task archive apply probe", &replacement, 1),
    )
    .expect("write hostile task text");
    close_inbox_task(&env);

    let preview = run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-d"]);
    assert_exit(&preview, 0, "human archive preview");
    assert!(!stdout(&preview).contains(hostile));
    assert!(!stdout(&preview).contains('\u{1b}'));

    let archived =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&archived, 0, "JSON archive report");
    let report = stdout(&archived);
    assert!(!report.contains(hostile));
    assert!(!report.contains("PRIVATE TASK TEXT"));
    assert!(!journal_text(&env).contains("PRIVATE TASK TEXT"));
}

// --- Section 2 (boundary) fault-isolation tests ---

fn thread_message_in(thread_id: &str, id: &str, labels: &[&str]) -> Value {
    json!({
        "id": id,
        "threadId": thread_id,
        "labelIds": labels,
        "internalDate": "1793908800000",
        "snippet": "A metadata-only archive fixture.",
        "payload": {
            "mimeType": "text/plain",
            "headers": [
                {"name": "From", "value": "Ada <ada@example.com>"},
                {"name": "To", "value": ACCOUNT},
                {"name": "Subject", "value": "Archive apply probe"},
                {"name": "Message-ID", "value": format!("<{id}@example.test>")}
            ],
            "body": {"size": 5, "data": "SGVsbG8="}
        }
    })
}

fn seed_threads(fake: &FakeGog, threads: Vec<(&str, Vec<Value>)>) {
    let mut thread_map = serde_json::Map::new();
    let mut message_map = serde_json::Map::new();
    for (thread_id, messages) in threads {
        thread_map.insert(
            thread_id.to_string(),
            json!({"thread": {"id": thread_id, "historyId": "1", "messages": messages}}),
        );
        for message in &messages {
            let id = message["id"].as_str().expect("message id").to_string();
            let label_ids = message["labelIds"].clone();
            let tid = message["threadId"]
                .as_str()
                .unwrap_or(thread_id)
                .to_string();
            message_map
                .insert(id, json!({"threadId": tid, "labelIds": label_ids}));
        }
    }
    fake.set_threads(Value::Object(thread_map));
    fake.set_messages(Value::Object(message_map));
}

fn file_adopt_thread(
    env: &GmailEnv,
    fake: &FakeGog,
    thread_id: &str,
) -> (String, String) {
    let saved = run_gmail(env, fake, &["save", thread_id, "-f", "json"]);
    assert_eq!(
        saved.status.code(),
        Some(0),
        "save the watched mail note:\n{}\n{}",
        stdout(&saved),
        stderr(&saved)
    );
    let doc = json_stdout(&saved);
    let note = doc["note"].as_str().expect("saved note").to_string();
    let link = doc["link"].as_str().expect("saved link").to_string();
    let inbox = env.vault().join("gmail_inbox.md");
    fs::write(
        &inbox,
        format!("#task archive apply probe {link}\n\n- [ ] #task archive apply probe {link}\n"),
    )
    .expect("write inbox task");
    let adopted =
        run_gmail(env, fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_eq!(
        adopted.status.code(),
        Some(0),
        "adopt the watched thread:\n{}\n{}",
        stdout(&adopted),
        stderr(&adopted)
    );
    (note, link)
}

#[test]
fn boundary_reopened_between_planning_and_mutation_holds_without_writing() {
    // Unique thread so the reopen seam cannot leak into parallel tests.
    const THREAD_R: &str = "cccccccccccccccc";
    const MESSAGE_R: &str = "cccccccccccccccc";
    let env = GmailEnv::new("bob-cli-gmail-archive-boundary-reopen");
    env.write_journal_rules();
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    seed_threads(
        &fake,
        vec![(
            THREAD_R,
            vec![thread_message_in(
                THREAD_R,
                MESSAGE_R,
                &["INBOX", ENROLLMENT_ID],
            )],
        )],
    );
    let (_note, _link) = file_adopt_thread(&env, &fake, THREAD_R);
    set_inbox_task_status(&env, 'x');
    fake.clear_calls();
    unsafe {
        std::env::set_var("BOB_GMAIL_ARCHIVE_TEST_REOPEN_THREAD", THREAD_R);
    }
    let held =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    unsafe {
        std::env::remove_var("BOB_GMAIL_ARCHIVE_TEST_REOPEN_THREAD");
    }
    assert_exit(&held, 1, "reopened evidence holds the run");
    let value = json_stdout(&held);
    assert!(
        value["archive"].as_array().expect("archive").is_empty(),
        "no terminal row on hold: {value}"
    );
    assert!(
        !journal_has(&env, "gmail.archived"),
        "no archive without a plannable terminal"
    );
    assert!(
        remote_write_calls(&fake).is_empty(),
        "held evidence writes nothing"
    );
}

#[test]
fn boundary_reply_between_read_and_batch_archives_only_frozen_ids() {
    const MESSAGE_REPLY: &str = "00000000000000c2";
    let env = GmailEnv::new("bob-cli-gmail-archive-boundary-race");
    env.write_journal_rules();
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    file_adopt(&env, &fake);
    close_inbox_task(&env);
    // A reply arrives between the last read and the batch. The thread envelope
    // the preview reads holds only the frozen ID; the messages map already
    // holds the reply without INBOX, and the race flips it to INBOX just
    // before the batch modify. The batch must touch only frozen IDs.
    fake.set_threads(json!({
        THREAD_A: {"thread": {"id": THREAD_A, "historyId": "1", "messages": [
            thread_message_in(THREAD_A, MESSAGE_A, &["INBOX", ENROLLMENT_ID]),
        ]}}
    }));
    fake.set_messages(json!({
        MESSAGE_A: {"threadId": THREAD_A, "labelIds": ["INBOX", ENROLLMENT_ID]},
        MESSAGE_REPLY: {"threadId": THREAD_A, "labelIds": [ENROLLMENT_ID]},
    }));
    fake.set_batch_race_before_modify(MESSAGE_REPLY, &["INBOX", ENROLLMENT_ID]);
    fake.clear_calls();
    // Re-arm the race after clearing the call log (clear_calls preserves state).
    fake.set_batch_race_before_modify(MESSAGE_REPLY, &["INBOX", ENROLLMENT_ID]);
    let raced =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&raced, 0, "race after read archives frozen only");
    let value = json_stdout(&raced);
    let rows = value["archive"].as_array().expect("archive");
    assert_eq!(rows[0]["action"], "archived");
    assert_eq!(rows[0]["messages"], json!([MESSAGE_A]));
    assert_eq!(
        fake.message_labels(MESSAGE_A),
        vec![RECEIPT_ID.to_string(), ENROLLMENT_ID.to_string()],
        "only frozen IDs change"
    );
    assert_eq!(
        fake.message_labels(MESSAGE_REPLY),
        vec!["INBOX".to_string(), ENROLLMENT_ID.to_string()],
        "the reply stays in INBOX"
    );
    assert!(journal_has(&env, "gmail.archived"));
}

#[test]
fn boundary_crash_between_let_go_write_and_journal_converges() {
    let env = GmailEnv::new("bob-cli-gmail-archive-boundary-letgo-crash");
    env.write_journal_rules();
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    file_adopt(&env, &fake);
    close_inbox_task(&env);
    let archived =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&archived, 0, "archive the closed thread");
    // A reply reopens INBOX membership; the next let-go will crash after its
    // remote write but before its journal append.
    seed_thread_and_messages(
        &fake,
        vec![
            thread_message(MESSAGE_A, &[RECEIPT_ID, ENROLLMENT_ID]),
            thread_message(MESSAGE_B, &["INBOX"]),
        ],
    );
    // Messages map needs both IDs with correct threadIds for the race-free path.
    seed_threads(
        &fake,
        vec![(
            THREAD_A,
            vec![
                thread_message_in(
                    THREAD_A,
                    MESSAGE_A,
                    &[RECEIPT_ID, ENROLLMENT_ID],
                ),
                thread_message_in(THREAD_A, MESSAGE_B, &["INBOX"]),
            ],
        )],
    );
    fake.clear_calls();
    fake.make_archive_journal_readonly_after_batch();
    let crashed =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&crashed, 1, "let-go journal failure fails the run");
    assert!(
        !journal_has(&env, "gmail.let_go"),
        "the failed local append claims nothing"
    );
    assert_eq!(
        fake.message_labels(MESSAGE_A),
        vec![ENROLLMENT_ID.to_string()],
        "the remote let-go succeeded before the crash"
    );
    // Restore journal writes; convergence records the release without another write.
    let journal = env.vault().join(".bob/gmail/test-host.jsonl");
    let mut permissions = fs::metadata(&journal)
        .expect("journal metadata")
        .permissions();
    permissions.set_readonly(false);
    fs::set_permissions(&journal, permissions).expect("restore journal writes");
    fake.clear_calls();
    let converged =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&converged, 0, "crash convergence records let-go");
    assert!(journal_has(&env, "gmail.let_go"));
    assert!(
        remote_write_calls(&fake).is_empty(),
        "convergence does not remutate"
    );
}

#[test]
fn boundary_recovery_after_crash_followed_by_reply_records_once() {
    const MESSAGE_NEW: &str = "00000000000000d3";
    let env = GmailEnv::new("bob-cli-gmail-archive-boundary-recover-reply");
    env.write_journal_rules();
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    file_adopt(&env, &fake);
    close_inbox_task(&env);
    fake.make_archive_journal_readonly_after_batch();
    let crashed =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&crashed, 1, "archive write succeeds then journal fails");
    assert!(!journal_has(&env, "gmail.archived"));
    let journal = env.vault().join(".bob/gmail/test-host.jsonl");
    let mut permissions = fs::metadata(&journal)
        .expect("journal metadata")
        .permissions();
    permissions.set_readonly(false);
    fs::set_permissions(&journal, permissions).expect("restore journal writes");
    // A reply (and a removed enrollment label stays) after the crash: the
    // thread now carries the receipt on the frozen ID plus a new unsaved reply.
    seed_threads(
        &fake,
        vec![(
            THREAD_A,
            vec![
                thread_message_in(
                    THREAD_A,
                    MESSAGE_A,
                    &[RECEIPT_ID, ENROLLMENT_ID],
                ),
                thread_message_in(
                    THREAD_A,
                    MESSAGE_NEW,
                    &["INBOX", ENROLLMENT_ID],
                ),
            ],
        )],
    );
    fake.clear_calls();
    let recovered =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&recovered, 0, "recovery records the frozen receipt once");
    let value = json_stdout(&recovered);
    assert_eq!(value["archive"][0]["action"], "archived");
    assert_eq!(value["archive"][0]["reason"], "recovered");
    assert_eq!(value["archive"][0]["messages"], json!([MESSAGE_A]));
    assert_eq!(
        fake.message_labels(MESSAGE_NEW),
        vec!["INBOX".to_string(), ENROLLMENT_ID.to_string()],
        "the reply stays in INBOX and is never touched"
    );
    assert!(
        remote_write_calls(&fake).is_empty(),
        "recovery does not remutate"
    );
}

#[test]
fn boundary_planned_kept_turning_archivable_is_held() {
    // A row planned Kept (unsaved reply visible at preview) that becomes
    // archivable before mutation must be held, never widening the batch.
    const THREAD_K: &str = "dddddddddddddddd";
    const MESSAGE_K: &str = "dddddddddddddddd";
    const MESSAGE_U: &str = "ddddddddddddddde";
    let env = GmailEnv::new("bob-cli-gmail-archive-boundary-planned-kept");
    env.write_journal_rules();
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    // Save with only the frozen ID; the unsaved reply arrives after the close
    // without a save, so it falls in U.
    seed_threads(
        &fake,
        vec![(
            THREAD_K,
            vec![thread_message_in(
                THREAD_K,
                MESSAGE_K,
                &["INBOX", ENROLLMENT_ID],
            )],
        )],
    );
    let (_note, _link) = file_adopt_thread(&env, &fake, THREAD_K);
    set_inbox_task_status(&env, 'x');
    // The reply arrives after the close (visible in Gmail, never saved).
    seed_threads(
        &fake,
        vec![(
            THREAD_K,
            vec![
                thread_message_in(
                    THREAD_K,
                    MESSAGE_K,
                    &["INBOX", ENROLLMENT_ID],
                ),
                thread_message_in(
                    THREAD_K,
                    MESSAGE_U,
                    &["INBOX", ENROLLMENT_ID],
                ),
            ],
        )],
    );
    // Preview plans Kept for the unsaved reply. The seam simulates the reply
    // disappearing between planning and mutation, so the fresh verdict turns
    // archivable: the planned-decision gate must hold without widening.
    unsafe {
        std::env::set_var("BOB_GMAIL_ARCHIVE_TEST_FORCE_ARCHIVABLE", THREAD_K);
    }
    let held =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    unsafe {
        std::env::remove_var("BOB_GMAIL_ARCHIVE_TEST_FORCE_ARCHIVABLE");
    }
    assert_exit(&held, 1, "upgrade after planning is held");
    let value = json_stdout(&held);
    assert!(
        value["archive"].as_array().expect("archive").is_empty(),
        "held upgrade reports no terminal row: {value}"
    );
    assert!(remote_write_calls(&fake).is_empty());
    assert!(!journal_has(&env, "gmail.archived"));
    assert!(!journal_has(&env, "gmail.kept"));
}

fn file_adopt_two(
    env: &GmailEnv,
    fake: &FakeGog,
    thread_a: &str,
    message_a: &str,
    thread_b: &str,
    message_b: &str,
) {
    seed_threads(
        fake,
        vec![
            (
                thread_a,
                vec![thread_message_in(
                    thread_a,
                    message_a,
                    &["INBOX", ENROLLMENT_ID],
                )],
            ),
            (
                thread_b,
                vec![thread_message_in(
                    thread_b,
                    message_b,
                    &["INBOX", ENROLLMENT_ID],
                )],
            ),
        ],
    );
    for thread_id in [thread_a, thread_b] {
        let saved = run_gmail(env, fake, &["save", thread_id, "-f", "json"]);
        assert_eq!(
            saved.status.code(),
            Some(0),
            "save {thread_id}:\n{}\n{}",
            stdout(&saved),
            stderr(&saved)
        );
    }
    // Re-read links from the two saves (save twice to capture both links).
    let inbox = env.vault().join("gmail_inbox.md");
    // Collect links by re-saving? Simpler: link tasks via thread IDs directly
    // is not supported; instead write two generic tasks and rely on the mail
    // index (saved notes) plus explicit links discovered from a second save.
    // For determinism, save once more to fetch links is unnecessary: write
    // tasks that reference each thread's note via a second save round-trip.
    let mut links = Vec::new();
    for thread_id in [thread_a, thread_b] {
        let saved = run_gmail(env, fake, &["save", thread_id, "-f", "json"]);
        let doc = json_stdout(&saved);
        links.push(doc["link"].as_str().expect("link").to_string());
    }
    let body = links
        .iter()
        .map(|link| format!("- [ ] #task archive apply probe {link}"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&inbox, format!("#task archive apply probe\n\n{body}\n"))
        .expect("write inbox tasks");
    let adopted =
        run_gmail(env, fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_eq!(
        adopted.status.code(),
        Some(0),
        "adopt both threads:\n{}\n{}",
        stdout(&adopted),
        stderr(&adopted)
    );
}

fn close_all_inbox_tasks(env: &GmailEnv) {
    let inbox = env.vault().join("gmail_inbox.md");
    let text = fs::read_to_string(&inbox).expect("read inbox");
    fs::write(&inbox, text.replace("- [ ]", "- [x]"))
        .expect("close inbox tasks");
}

#[test]
fn boundary_two_threads_where_one_batch_fails_isolates() {
    const THREAD_C: &str = "bbbbbbbbbbbbbbbb";
    const MESSAGE_C: &str = "bbbbbbbbbbbbbbbb";
    let env = GmailEnv::new("bob-cli-gmail-archive-boundary-two-fail");
    env.write_journal_rules();
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    file_adopt_two(&env, &fake, THREAD_A, MESSAGE_A, THREAD_C, MESSAGE_C);
    close_all_inbox_tasks(&env);
    // First batch (THREAD_A, sorted first) succeeds; second fails transiently.
    fake.set_fail_after(
        "batch_modify",
        "transient: PRIVATE_SENTINEL flap",
        1,
        1,
    );
    fake.clear_calls();
    // Re-arm after clearing the log (state is preserved).
    fake.set_fail_after(
        "batch_modify",
        "transient: PRIVATE_SENTINEL flap",
        1,
        1,
    );
    let failed =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&failed, 1, "one failure fails the run without rollback");
    let value = json_stdout(&failed);
    let rows = value["archive"].as_array().expect("archive");
    assert_eq!(rows.len(), 1, "only the successful thread reports: {value}");
    assert_eq!(rows[0]["thread_id"], THREAD_A);
    assert_eq!(rows[0]["action"], "archived");
    assert!(journal_has(&env, "gmail.archive_closed"));
    // The successful thread's filing and archive are kept; the failed thread
    // consumes nothing.
    assert!(env.vault().join("gmail_inbox.md").is_file());
}

#[test]
fn boundary_invalid_grant_on_second_thread_aborts_later_writes() {
    let env = GmailEnv::new("bob-cli-gmail-archive-boundary-invalid-grant");
    env.write_journal_rules();
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    // Run 1: archive THREAD_A to create an owned receipt.
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    file_adopt(&env, &fake);
    close_inbox_task(&env);
    let first =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&first, 0, "first archive succeeds");
    assert!(journal_has(&env, "gmail.archived"));
    // Run 2: THREAD_B needs archiving; THREAD_A needs let-go via a reply.
    // THREAD_B's metadata fetch fails with invalid_grant, aborting before
    // any later write (including let-go and label creation).
    const THREAD_D: &str = "ddddddddddddddd0";
    const MESSAGE_D: &str = "ddddddddddddddd0";
    seed_threads(
        &fake,
        vec![
            (
                THREAD_A,
                vec![
                    thread_message_in(
                        THREAD_A,
                        MESSAGE_A,
                        &[RECEIPT_ID, ENROLLMENT_ID],
                    ),
                    thread_message_in(
                        THREAD_A,
                        MESSAGE_B,
                        &["INBOX", ENROLLMENT_ID],
                    ),
                ],
            ),
            (
                THREAD_D,
                vec![thread_message_in(
                    THREAD_D,
                    MESSAGE_D,
                    &["INBOX", ENROLLMENT_ID],
                )],
            ),
        ],
    );
    // Adopt THREAD_D (save + link + close) without disturbing THREAD_A's note.
    let saved = run_gmail(&env, &fake, &["save", THREAD_D, "-f", "json"]);
    assert_eq!(saved.status.code(), Some(0), "save second thread");
    let link = json_stdout(&saved)["link"]
        .as_str()
        .expect("link")
        .to_string();
    let inbox = env.vault().join("gmail_inbox.md");
    let mut text = fs::read_to_string(&inbox).expect("read inbox");
    text.push_str(&format!("\n- [ ] #task archive apply probe {link}\n"));
    fs::write(&inbox, &text).expect("add second task");
    // Adopt with -n to skip archive (and let-go), preserving A's let-go for
    // the final aborted run.
    let adopted = run_gmail(
        &env,
        &fake,
        &["sync", "-l", "0", "-r", "0", "-n", "-f", "json"],
    );
    assert_exit(&adopted, 0, "adopt second thread without archiving");
    // Close the second thread's task (last task row).
    let text = fs::read_to_string(&inbox).expect("read inbox");
    // Close only the newly added (last) open row.
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    if let Some(pos) = lines.iter().rposition(|l| l.starts_with("- [ ]")) {
        lines[pos] = lines[pos].replacen("- [ ]", "- [x]", 1);
    }
    fs::write(&inbox, lines.join("\n") + "\n").expect("close second task");
    fake.set_thread_fail(THREAD_D, "invalid_grant: expired", 1);
    fake.clear_calls();
    let aborted =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&aborted, 1, "invalid_grant aborts the run");
    // First thread's filing and archive from run 1 are kept.
    assert!(journal_has(&env, "gmail.archived"));
    assert!(env.vault().join("gmail_inbox.md").is_file());
    // The abort happens before release_owned, so A's pending let-go never runs.
    assert!(!journal_has(&env, "gmail.let_go"), "abort stops let-go");
    // No let-go was recorded for THREAD_A in the aborted run (would need a
    // second journal entry); the abort stopped later writes.
    let writes = remote_write_calls(&fake);
    assert!(
        writes
            .iter()
            .all(|call| !call.contains(&"gmail.labels.create".to_string())),
        "no label creation after the abort: {writes:?}"
    );
}

#[test]
fn boundary_let_go_runs_while_backlog_gate_is_paused() {
    let env = GmailEnv::new("bob-cli-gmail-archive-boundary-paused-letgo");
    env.write_journal_rules();
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    // Run 1: archive THREAD_A to create an owned receipt.
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    file_adopt(&env, &fake);
    close_inbox_task(&env);
    let first =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&first, 0, "first archive succeeds");
    // THREAD_A now needs let-go via a reply; THREAD_B becomes a paused backlog
    // (adopted with -n so its first close is historical and needs -c).
    seed_threads(
        &fake,
        vec![
            (
                THREAD_A,
                vec![
                    thread_message_in(
                        THREAD_A,
                        MESSAGE_A,
                        &[RECEIPT_ID, ENROLLMENT_ID],
                    ),
                    thread_message_in(
                        THREAD_A,
                        MESSAGE_B,
                        &["INBOX", ENROLLMENT_ID],
                    ),
                ],
            ),
            (
                THREAD_B,
                vec![thread_message_in(
                    THREAD_B,
                    MESSAGE_C_BLANK,
                    &["INBOX", ENROLLMENT_ID],
                )],
            ),
        ],
    );
    // Adopt THREAD_B without archive observations so its first close is a
    // historical backlog. Use -n to skip archive (and let-go) during adoption.
    let saved_b = run_gmail(&env, &fake, &["save", THREAD_B, "-f", "json"]);
    assert_eq!(saved_b.status.code(), Some(0), "save backlog thread");
    let link_b = json_stdout(&saved_b)["link"]
        .as_str()
        .expect("link")
        .to_string();
    let inbox = env.vault().join("gmail_inbox.md");
    let mut text = fs::read_to_string(&inbox).expect("read inbox");
    text.push_str(&format!("\n- [ ] #task archive apply probe {link_b}\n"));
    fs::write(&inbox, &text).expect("add backlog task");
    let adopted_b = run_gmail(
        &env,
        &fake,
        &["sync", "-l", "0", "-r", "0", "-n", "-f", "json"],
    );
    assert_exit(&adopted_b, 0, "adopt backlog thread without archiving");
    // Close the backlog thread's task (last open row).
    let text = fs::read_to_string(&inbox).expect("read inbox");
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    if let Some(pos) = lines.iter().rposition(|l| l.starts_with("- [ ]")) {
        lines[pos] = lines[pos].replacen("- [ ]", "- [x]", 1);
    }
    fs::write(&inbox, lines.join("\n") + "\n").expect("close backlog task");
    fake.clear_calls();
    let paused =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&paused, 1, "backlog gate pauses the run");
    let value = json_stdout(&paused);
    let rows = value["archive"].as_array().expect("archive rows");
    // One paused backlog row and one let-go row.
    assert!(
        rows.iter().any(|row| row["thread_id"] == THREAD_B
            && row["action"] == "paused"
            && row["reason"] == "backlog_unconfirmed"),
        "backlog row is paused: {value}"
    );
    assert!(
        rows.iter().any(
            |row| row["thread_id"] == THREAD_A && row["action"] == "let_go"
        ),
        "let-go still runs while paused: {value}"
    );
    assert!(journal_has(&env, "gmail.let_go"), "let-go journaled");
    assert!(
        !journal_text(&env).contains(
            "\"thread_id\":\"aaaaaaaaaaaaaaaa\",\"action\":\"archived\""
        ) && !value["archive"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["thread_id"] == THREAD_B
                && row["action"] == "archived"),
        "paused backlog never archives"
    );
    let writes = remote_write_calls(&fake);
    assert!(
        writes
            .iter()
            .any(|call| call.contains(&"gmail.batch.modify".to_string())),
        "let-go batch ran while paused: {writes:?}"
    );
    assert!(
        !writes
            .iter()
            .any(|call| call.contains(&"gmail.labels.create".to_string())),
        "paused creates no labels: {writes:?}"
    );
}

const MESSAGE_C_BLANK: &str = "00000000000000e4";
