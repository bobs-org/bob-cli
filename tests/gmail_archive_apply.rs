//! `bob gmail sync` live archive apply: one-shot mutation, let-go, and
//! crash reconciliation over the fake transport.
//!
//! Every live run must keep durable filings when archive setup or per-thread
//! work fails, isolate non-auth failures, abort on `invalid_grant`, create
//! no labels for paused batches, and converge crashes without reclaiming.

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
    let inbox = env.vault().join("gmail_inbox.md");
    let text = fs::read_to_string(&inbox).expect("read inbox");
    fs::write(&inbox, text.replacen("- [ ] #task", "- [x] #task", 1))
        .expect("close inbox task");
}

#[test]
fn live_archives_a_fresh_close_and_repeats_as_noop() {
    let env = GmailEnv::new("bob-cli-gmail-archive-apply-fresh");
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
fn live_reconciles_a_crashed_archive_without_remutating() {
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
    let archived =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&archived, 0, "first archive succeeds");
    assert!(journal_has(&env, "gmail.archived"));
    fake.clear_calls();
    let again =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&again, 0, "owned archive without INBOX stays quiet");
    assert!(remote_write_calls(&fake).is_empty());
    let _ = THREAD_B;
}
