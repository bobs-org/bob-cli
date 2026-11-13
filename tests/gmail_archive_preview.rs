//! `bob gmail sync -d` archive preview: read-only planning over the fake
//! transport, canonical digests, backlog/burst gates, and `-c`/`-n` parsing.
//!
//! Every preview run must leave the vault, the journals, and Gmail
//! untouched: tests hash the vault before and after, compare journal kinds,
//! and assert the fake recorded no `batch.modify` or `labels.create` calls.

mod gmail_support;

use std::fs;

use gmail_support::{
    json_stdout, run_gmail, stderr, stdout, FakeGog, GmailEnv, ACCOUNT, HOST,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const WATCH_THREAD: &str = "0123456789abcdef";
const MESSAGE_ID: &str = "00000000000000a0";
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

fn write_archive_config(env: &GmailEnv, extra: &str) {
    env.write_config(&format!(
        "timezone: America/New_York\ngmail:\n  account: {ACCOUNT}\n  writer: {HOST}\n  sync:\n    archive: on\n{extra}"
    ));
}

fn write_task_settings(env: &GmailEnv) {
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

fn hash_value(value: &Value) -> String {
    format!(
        "sha256:{}",
        hex::encode(Sha256::digest(serde_json::to_vec(value).unwrap()))
    )
}

/// The first enablement event, exactly as the apply phase records it:
/// name-bound receipt label, no previous change.
fn enablement_line() -> String {
    let label = json!({"name": "bob/closed", "id": Value::Null});
    let epoch = hash_value(&json!({
        "purpose": "gmail.archive.epoch",
        "account": ACCOUNT,
        "previous_change_id": Value::Null,
        "label": label,
    }));
    let change_id = hash_value(&json!({
        "purpose": "gmail.archive.enablement",
        "account": ACCOUNT,
        "enabled": true,
        "epoch": epoch,
        "previous_change_id": Value::Null,
        "label": label,
    }));
    serde_json::to_string(&json!({
        "v": 1,
        "ts": "2026-01-05T09:00:00-05:00",
        "host": "athena",
        "kind": "gmail.archive_enabled",
        "value": {
            "account": ACCOUNT,
            "enabled": true,
            "epoch": epoch,
            "change_id": change_id,
            "previous_change_id": Value::Null,
            "label": label,
        },
    }))
    .unwrap()
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
        "threadId": WATCH_THREAD,
        "labelIds": labels,
        "internalDate": "1793908800000",
        "snippet": "A metadata-only archive fixture.",
        "payload": {
            "mimeType": "text/plain",
            "headers": [
                {"name": "From", "value": "Ada <ada@example.com>"},
                {"name": "To", "value": ACCOUNT},
                {"name": "Subject", "value": "Archive preview probe"},
                {"name": "Message-ID", "value": format!("<{id}@example.test>")}
            ],
            "body": {"size": 5, "data": "SGVsbG8="}
        }
    })
}

fn seed_thread(fake: &FakeGog, messages: Vec<Value>) {
    fake.set_threads(json!({
        WATCH_THREAD: {"thread": {"id": WATCH_THREAD, "historyId": "1", "messages": messages}}
    }));
}

fn journal_kinds(env: &GmailEnv) -> Vec<String> {
    let dir = env.vault().join(".bob/gmail");
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut kinds: Vec<String> = entries
        .flatten()
        .filter_map(|entry| fs::read_to_string(entry.path()).ok())
        .flat_map(|text| {
            text.lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .filter_map(|event| event["kind"].as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .collect();
    kinds.sort();
    kinds
}

fn snapshot_vault(env: &GmailEnv) -> Vec<(String, Vec<u8>)> {
    fn visit(
        root: &std::path::Path,
        current: &std::path::Path,
        out: &mut Vec<(String, Vec<u8>)>,
    ) {
        let Ok(entries) = fs::read_dir(current) else {
            return;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .expect("relative fixture path")
                .to_string_lossy()
                .replace('\\', "/");
            if path.is_dir() {
                out.push((relative, Vec::new()));
                visit(root, &path, out);
            } else if let Ok(bytes) = fs::read(&path) {
                out.push((relative, bytes));
            }
        }
    }
    let mut out = Vec::new();
    visit(env.vault(), env.vault(), &mut out);
    out.sort_by(|left, right| left.0.cmp(&right.0));
    out
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

/// File one thread through save + adopt, then close its only task.
fn file_and_close(env: &GmailEnv, fake: &FakeGog) -> (String, String) {
    let saved = run_gmail(env, fake, &["save", WATCH_THREAD, "-f", "json"]);
    assert_eq!(
        saved.status.code(),
        Some(0),
        "save the watched mail note:\n{}\n{}",
        stdout(&saved),
        stderr(&saved)
    );
    let doc = json_stdout(&saved);
    let note = doc["note"].as_str().expect("saved note path").to_string();
    let link = doc["link"].as_str().expect("saved note link").to_string();

    let inbox = env.vault().join("gmail_inbox.md");
    fs::write(
        &inbox,
        format!(
            "---\ntype: \"[[inbox]]\"\nready_cap: off\n---\n\n# Gmail Inbox\n\n## Tasks\n- [ ] #task archive preview probe phrase {link}\n"
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

    let text = fs::read_to_string(&inbox).expect("read inbox");
    fs::write(&inbox, text.replacen("- [ ] #task", "- [x] #task", 1))
        .expect("close the inbox task");
    (note, link)
}

#[test]
fn dry_run_pauses_backlog_and_confirms_with_the_digest() {
    let env = GmailEnv::new("bob-cli-gmail-archive-preview");
    write_archive_config(&env, "");
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog");
    seed_labels(&fake, true);
    seed_thread(
        &fake,
        vec![thread_message(MESSAGE_ID, &["INBOX", ENROLLMENT_ID])],
    );

    let journal_dir = env.vault().join(".bob/gmail");
    fs::create_dir_all(&journal_dir).expect("create journal dir");
    fs::write(journal_dir.join("athena.jsonl"), enablement_line() + "\n")
        .expect("seed archive enablement");

    let (note, _) = file_and_close(&env, &fake);

    let before_vault = snapshot_vault(&env);
    let before_journal = journal_kinds(&env);

    let paused = run_gmail(&env, &fake, &["sync", "-d", "-f", "json"]);
    assert_exit(&paused, 1, "unconfirmed backlog pauses");
    let value = json_stdout(&paused);
    assert_eq!(value["dry_run"], true);
    let archive = value["archive"].as_array().expect("archive rows");
    assert_eq!(archive.len(), 1);
    assert_eq!(archive[0]["thread_id"], WATCH_THREAD);
    assert_eq!(archive[0]["note"], Value::String(note.clone()));
    assert_eq!(archive[0]["action"], "paused");
    assert_eq!(archive[0]["reason"], "backlog_unconfirmed");
    assert_eq!(archive[0]["messages"], json!([MESSAGE_ID]));
    assert_eq!(archive[0]["tasks"][0]["path"], "gmail_inbox.md");
    assert!(archive[0]["tasks"][0]["line"].is_number());
    let digest = value["archive_digest"]
        .as_str()
        .expect("digest")
        .to_string();
    assert_eq!(digest.len(), 64);
    assert!(digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert!(
        !stderr(&paused).contains("skipped archive preview"),
        "the enablement fixture must parse:\n{}",
        stderr(&paused)
    );

    assert_eq!(snapshot_vault(&env), before_vault, "preview writes nothing");
    assert_eq!(
        journal_kinds(&env),
        before_journal,
        "preview appends nothing"
    );
    assert!(
        remote_write_calls(&fake).is_empty(),
        "preview performs no remote writes"
    );

    let human = run_gmail(&env, &fake, &["sync", "-d"]);
    assert_exit(&human, 1, "human backlog pauses");
    let text = stdout(&human);
    assert!(text.contains("paused"), "{text}");
    assert!(text.contains("backlog_unconfirmed"), "{text}");
    assert!(text.contains(&digest), "{text}");
    assert!(text.contains("confirm with:"), "{text}");
    assert!(
        !text.contains("archive preview probe phrase"),
        "task prose stays out of the report:\n{text}"
    );

    let confirmed =
        run_gmail(&env, &fake, &["sync", "-d", "-c", &digest, "-f", "json"]);
    assert_exit(&confirmed, 0, "matching digest confirms the preview");
    let value = json_stdout(&confirmed);
    assert_eq!(value["archive"][0]["action"], "would_archive");
    assert_eq!(value["archive"][0]["reason"], "backlog");
    assert_eq!(value["archive"][0]["messages"], json!([MESSAGE_ID]));
    assert_eq!(value["archive_digest"].as_str(), Some(digest.as_str()));
    assert_eq!(snapshot_vault(&env), before_vault, "confirm writes nothing");
    assert!(remote_write_calls(&fake).is_empty());

    let stale_digest = "0".repeat(64);
    let stale = run_gmail(
        &env,
        &fake,
        &["sync", "-d", "-c", &stale_digest, "-f", "json"],
    );
    assert_exit(&stale, 1, "stale digest is a typed mismatch");
    let value = json_stdout(&stale);
    assert_eq!(value["archive"][0]["action"], "paused");
    assert_eq!(value["archive"][0]["reason"], "confirm_mismatch");
    assert!(
        value["warnings"].as_array().is_some_and(|warnings| {
            warnings.iter().any(|warning| {
                warning
                    .as_str()
                    .is_some_and(|warning| warning.contains("does not match"))
            })
        }),
        "mismatch is loud:\n{}",
        serde_json::to_string_pretty(&value).unwrap()
    );

    let limited =
        run_gmail(&env, &fake, &["sync", "-d", "-l", "0", "-f", "json"]);
    assert_exit(&limited, 1, "-l 0 still plans enrolled threads");
    assert_eq!(
        json_stdout(&limited)["archive"][0]["reason"],
        "backlog_unconfirmed"
    );

    let skipped = run_gmail(&env, &fake, &["sync", "-d", "-n", "-f", "json"]);
    assert_exit(&skipped, 0, "-n skips planning");
    let value = json_stdout(&skipped);
    assert!(value["archive"].as_array().unwrap().is_empty());
    assert!(value.get("archive_digest").is_none());
    assert_eq!(journal_kinds(&env), before_journal, "-n consumes nothing");
}

#[test]
fn dry_run_keeps_threads_with_unsaved_inbox_mail() {
    let env = GmailEnv::new("bob-cli-gmail-archive-unsaved");
    write_archive_config(&env, "");
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog");
    seed_labels(&fake, true);
    seed_thread(
        &fake,
        vec![thread_message(MESSAGE_ID, &["INBOX", ENROLLMENT_ID])],
    );

    let journal_dir = env.vault().join(".bob/gmail");
    fs::create_dir_all(&journal_dir).expect("create journal dir");
    fs::write(journal_dir.join("athena.jsonl"), enablement_line() + "\n")
        .expect("seed archive enablement");

    file_and_close(&env, &fake);

    seed_thread(
        &fake,
        vec![
            thread_message(MESSAGE_ID, &["INBOX", ENROLLMENT_ID]),
            thread_message("00000000000000b1", &["INBOX", ENROLLMENT_ID]),
        ],
    );

    let paused = run_gmail(&env, &fake, &["sync", "-d", "-f", "json"]);
    assert_exit(&paused, 1, "unsaved backlog still pauses");
    let digest = json_stdout(&paused)["archive_digest"]
        .as_str()
        .expect("digest")
        .to_string();

    let confirmed =
        run_gmail(&env, &fake, &["sync", "-d", "-c", &digest, "-f", "json"]);
    assert_exit(&confirmed, 0, "confirmed preview reports the keep");
    let value = json_stdout(&confirmed);
    assert_eq!(value["archive"][0]["action"], "kept");
    assert_eq!(value["archive"][0]["reason"], "unsaved");
    assert_eq!(value["archive"][0]["messages"], json!([MESSAGE_ID]));
}

#[test]
fn dry_run_plans_without_the_remote_receipt_label() {
    let env = GmailEnv::new("bob-cli-gmail-archive-no-receipt");
    write_archive_config(&env, "");
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog");
    seed_labels(&fake, false);
    seed_thread(
        &fake,
        vec![thread_message(MESSAGE_ID, &["INBOX", ENROLLMENT_ID])],
    );

    let journal_dir = env.vault().join(".bob/gmail");
    fs::create_dir_all(&journal_dir).expect("create journal dir");
    fs::write(journal_dir.join("athena.jsonl"), enablement_line() + "\n")
        .expect("seed archive enablement");

    file_and_close(&env, &fake);

    let paused = run_gmail(&env, &fake, &["sync", "-d", "-f", "json"]);
    assert_exit(&paused, 1, "a missing receipt label still pauses");
    let value = json_stdout(&paused);
    assert_eq!(value["archive"][0]["reason"], "backlog_unconfirmed");
    assert!(
        value["archive_digest"]
            .as_str()
            .is_some_and(|digest| digest.len() == 64),
        "the digest binds the receipt name, not its remote id"
    );
    assert!(remote_write_calls(&fake).is_empty());
}
