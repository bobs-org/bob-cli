//! Mail-note turn fields and the `mail.base` Waiting for reply view.
//!
//! The fake gog answers `gmail thread get` with Gmail's full-format JSON, as
//! real gog does: every message carries all of its payload headers. These
//! tests pin the headers the turn classifier needs by driving each one
//! through `bob gmail save`.

mod gmail_support;

use std::{fs, process::Command};

use gmail_support::{
    json_stdout, run_gmail, stderr, stdout, FakeGog, GmailEnv, ACCOUNT,
};
use serde_json::{json, Value};

const THREAD: &str = "0123456789abcdef";
const DANA: &str = "Dana <dana@example.test>";
/// 2026-11-06T10:00:00Z, five minutes per message after it.
const FIRST_MILLIS: u64 = 1_793_959_200_000;

const MAIL_BASE_V1: &str = "filters:\n  and:\n    - 'content == \"untrusted-email\"'\nproperties:\n  title:\n    displayName: Subject\nviews:\n  - type: table\n    name: Mail\n    order:\n      - title\n      - who\n      - started\n      - updated\n      - messages\n      - file.backlinks\n";

fn assert_success(output: &std::process::Output, label: &str) {
    assert_eq!(
        output.status.code(),
        Some(0),
        "{label} expected exit 0:\nstdout:\n{}\nstderr:\n{}",
        stdout(output),
        stderr(output)
    );
}

fn env(prefix: &str) -> GmailEnv {
    let env = GmailEnv::new(prefix);
    env.write_config(&format!(
        "timezone: America/New_York\ngmail:\n  account: {ACCOUNT}\n  writer: test-host\n"
    ));
    env
}

/// One message `index` (0-based) with `labels` and extra `headers`.
fn message(index: u64, labels: &[&str], headers: &[(&str, &str)]) -> Value {
    let mut all = vec![
        json!({"name": "Subject", "value": "Lease renewal"}),
        json!({"name": "Message-ID", "value": format!("<m{index}@example.test>")}),
    ];
    all.extend(
        headers
            .iter()
            .map(|(name, value)| json!({"name": name, "value": value})),
    );
    json!({
        "id": format!("{:016x}", 0xa0 + index),
        "threadId": THREAD,
        "labelIds": labels,
        "internalDate": (FIRST_MILLIS + index * 300_000).to_string(),
        "snippet": "Hello",
        "payload": {
            "mimeType": "text/plain",
            "headers": all,
            "body": {"size": 5, "data": "SGVsbG8="}
        }
    })
}

fn your_send(index: u64, headers: &[(&str, &str)]) -> Value {
    let mut all = vec![("From", "Bryan <you@example.com>")];
    all.extend_from_slice(headers);
    message(index, &["SENT"], &all)
}

fn their_reply(index: u64, extra: &[(&str, &str)]) -> Value {
    let mut all = vec![("From", DANA), ("To", ACCOUNT)];
    all.extend_from_slice(extra);
    message(index, &["INBOX"], &all)
}

fn set_messages(fake: &FakeGog, messages: Vec<Value>) {
    fake.set_thread(
        json!({"thread": {"id": THREAD, "historyId": "7", "messages": messages}}),
        json!({}),
    );
}

fn save(env: &GmailEnv, fake: &FakeGog) -> Value {
    let output = run_gmail(env, fake, &["save", THREAD, "-f", "json"]);
    assert_success(&output, "gmail save");
    json_stdout(&output)
}

fn frontmatter(env: &GmailEnv, note: &str) -> serde_yaml::Mapping {
    let text = fs::read_to_string(env.vault().join(note)).expect("mail note");
    let yaml = text
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n"))
        .map(|(yaml, _)| yaml)
        .expect("frontmatter");
    serde_yaml::from_str(yaml).expect("frontmatter yaml")
}

fn field(mapping: &serde_yaml::Mapping, key: &str) -> Option<String> {
    mapping
        .get(serde_yaml::Value::String(key.to_string()))
        .and_then(serde_yaml::Value::as_str)
        .map(str::to_string)
}

/// Each header the classifier reads, delivered through gog's JSON envelope,
/// decides the saved turn.
#[test]
fn every_classifier_header_reaches_the_saved_turn() {
    let cases: Vec<(&str, Vec<Value>, Option<&str>)> = vec![
        ("from", vec![their_reply(0, &[])], Some("yours")),
        ("to", vec![your_send(0, &[("To", DANA)])], Some("theirs")),
        (
            "cc",
            vec![your_send(0, &[("To", ACCOUNT), ("Cc", DANA)])],
            Some("theirs"),
        ),
        (
            "bcc",
            vec![your_send(0, &[("To", ACCOUNT), ("Bcc", DANA)])],
            Some("theirs"),
        ),
        ("self", vec![your_send(0, &[("To", ACCOUNT)])], None),
        (
            "auto-submitted",
            vec![
                your_send(0, &[("To", DANA)]),
                their_reply(1, &[("Auto-Submitted", "auto-replied")]),
            ],
            Some("theirs"),
        ),
        (
            "auto-submitted-no",
            vec![
                your_send(0, &[("To", DANA)]),
                their_reply(1, &[("Auto-Submitted", "no")]),
            ],
            Some("yours"),
        ),
        (
            "x-autoreply",
            vec![
                your_send(0, &[("To", DANA)]),
                their_reply(1, &[("X-Autoreply", "yes")]),
            ],
            Some("theirs"),
        ),
        (
            "x-autorespond",
            vec![
                your_send(0, &[("To", DANA)]),
                their_reply(1, &[("X-Autorespond", "yes")]),
            ],
            Some("theirs"),
        ),
        (
            "precedence",
            vec![
                your_send(0, &[("To", DANA)]),
                their_reply(1, &[("Precedence", "auto_reply")]),
            ],
            Some("theirs"),
        ),
        (
            "content-type",
            vec![
                your_send(0, &[("To", DANA)]),
                message(
                    1,
                    &["INBOX"],
                    &[
                        ("From", "Relay <relay@example.test>"),
                        ("To", ACCOUNT),
                        (
                            "Content-Type",
                            "multipart/report; report-type=delivery-status; boundary=\"b\"",
                        ),
                    ],
                ),
            ],
            Some("yours"),
        ),
    ];
    for (name, messages, expected) in cases {
        let env = env(&format!("bob-cli-gmail-turn-{name}"));
        let fake = FakeGog::new(&env, "gog");
        set_messages(&fake, messages);
        let document = save(&env, &fake);
        let note = document["note"].as_str().expect("note path");
        let fields = frontmatter(&env, note);
        assert_eq!(field(&fields, "turn").as_deref(), expected, "{name}");
        assert_eq!(
            field(&fields, "turn_since").is_some(),
            expected.is_some(),
            "{name}"
        );
        let gets = fake
            .calls()
            .into_iter()
            .filter(|call| {
                call.windows(3).any(|w| w == ["gmail", "thread", "get"])
            })
            .collect::<Vec<_>>();
        assert!(!gets.is_empty(), "{name}");
        assert!(
            gets.iter()
                .all(|call| !call.iter().any(|arg| arg == "--format")),
            "{name}: gog's thread get has no --format"
        );
    }
}

#[test]
fn turn_follows_messages_in_the_note_and_list_json() {
    let env = env("bob-cli-gmail-turn-list");
    let fake = FakeGog::new(&env, "gog");
    set_messages(
        &fake,
        vec![their_reply(0, &[]), your_send(1, &[("To", DANA)])],
    );
    let document = save(&env, &fake);
    let note = document["note"].as_str().expect("note path").to_string();
    let text = fs::read_to_string(env.vault().join(&note)).unwrap();
    assert!(
        text.contains(
            "messages: 2\nturn: theirs\nturn_since: 2026-11-06T05:05:00-05:00\ngmail_account:"
        ),
        "{text}"
    );

    let output = run_gmail(&env, &fake, &["list", "-f", "json"]);
    assert_success(&output, "gmail list");
    let list = json_stdout(&output);
    assert_eq!(list["threads"][0]["turn"], "theirs");
    assert_eq!(
        list["threads"][0]["turn_since"],
        "2026-11-06T05:05:00-05:00"
    );

    // Their reply flips the turn on refresh.
    set_messages(
        &fake,
        vec![
            their_reply(0, &[]),
            your_send(1, &[("To", DANA)]),
            their_reply(2, &[]),
        ],
    );
    let document = save(&env, &fake);
    assert_eq!(document["results"][0]["status"], "refreshed");
    let fields = frontmatter(&env, &note);
    assert_eq!(field(&fields, "turn").as_deref(), Some("yours"));
    assert_eq!(
        field(&fields, "turn_since").as_deref(),
        Some("2026-11-06T05:10:00-05:00")
    );

    // A note saved before the turn fields lists them as null, and a refresh
    // with no new message still writes them.
    let legacy = fs::read_to_string(env.vault().join(&note))
        .unwrap()
        .lines()
        .filter(|line| !line.starts_with("turn"))
        .map(|line| format!("{line}\n"))
        .collect::<String>();
    fs::write(env.vault().join(&note), legacy).unwrap();
    let output = run_gmail(&env, &fake, &["list", "-f", "json"]);
    let list = json_stdout(&output);
    assert_eq!(list["threads"][0]["turn"], Value::Null);
    assert_eq!(list["threads"][0]["turn_since"], Value::Null);
    let dry = run_gmail(&env, &fake, &["save", THREAD, "-d", "-f", "json"]);
    assert_success(&dry, "gmail save dry run");
    let preview = json_stdout(&dry)["results"][0]["preview"].to_string();
    assert!(
        preview.contains("~ owned metadata fields: turn, turn_since"),
        "{preview}"
    );
    let document = save(&env, &fake);
    assert_eq!(document["results"][0]["status"], "refreshed");
    assert_eq!(
        field(&frontmatter(&env, &note), "turn").as_deref(),
        Some("yours")
    );
}

#[test]
fn a_missing_mail_base_is_created_with_the_waiting_for_reply_view() {
    let env = env("bob-cli-gmail-turn-base-create");
    let fake = FakeGog::new(&env, "gog");
    set_messages(&fake, vec![their_reply(0, &[])]);
    save(&env, &fake);
    let base = fs::read_to_string(env.vault().join("mail.base")).unwrap();
    assert!(base.starts_with(MAIL_BASE_V1));
    assert!(base.contains("    name: Waiting for reply\n"));
    assert!(base.contains("        - 'turn == \"theirs\"'\n"));
    assert!(
        base.contains("      - property: turn_since\n        direction: ASC\n")
    );
}

#[test]
fn an_untouched_mail_base_upgrades_only_with_a_note_write_and_commits() {
    let env = env("bob-cli-gmail-turn-base-upgrade");
    initialize_git_vault(env.vault());
    fs::write(env.vault().join("mail.base"), MAIL_BASE_V1).unwrap();
    assert!(git(env.vault(), &["add", "mail.base"]).status.success());
    assert!(git(env.vault(), &["commit", "-q", "-m", "v1 base"])
        .status
        .success());
    let fake = FakeGog::new(&env, "gog");
    set_messages(&fake, vec![their_reply(0, &[])]);
    save(&env, &fake);
    let base = fs::read_to_string(env.vault().join("mail.base")).unwrap();
    assert!(base.contains("Waiting for reply"), "{base}");
    let status =
        git(env.vault(), &["status", "--porcelain", "--", "mail.base"]);
    assert!(stdout(&status).trim().is_empty(), "{}", stdout(&status));
    let shown = git(env.vault(), &["show", "--name-only", "--format=", "HEAD"]);
    assert!(stdout(&shown).contains("mail.base"), "{}", stdout(&shown));

    // Restore the old base: an unchanged save writes no note, so it leaves
    // the base alone.
    fs::write(env.vault().join("mail.base"), MAIL_BASE_V1).unwrap();
    let document = save(&env, &fake);
    assert_eq!(document["results"][0]["status"], "unchanged");
    assert_eq!(
        fs::read_to_string(env.vault().join("mail.base")).unwrap(),
        MAIL_BASE_V1
    );
}

#[test]
fn a_customized_mail_base_is_never_touched() {
    let env = env("bob-cli-gmail-turn-base-custom");
    let custom = MAIL_BASE_V1.replace("Subject", "Topic");
    fs::write(env.vault().join("mail.base"), &custom).unwrap();
    let fake = FakeGog::new(&env, "gog");
    set_messages(&fake, vec![their_reply(0, &[])]);
    let document = save(&env, &fake);
    assert_eq!(document["results"][0]["status"], "created");
    assert_eq!(
        fs::read_to_string(env.vault().join("mail.base")).unwrap(),
        custom
    );
}

fn initialize_git_vault(vault: &std::path::Path) {
    assert!(git(vault, &["init", "-q"]).status.success());
    assert!(git(vault, &["config", "user.name", "Bob Test"])
        .status
        .success());
    assert!(git(vault, &["config", "user.email", "bob@example.test"])
        .status
        .success());
    fs::write(vault.join("base.md"), "base\n").unwrap();
    assert!(git(vault, &["add", "base.md"]).status.success());
    assert!(git(vault, &["commit", "-q", "-m", "base"]).status.success());
}

fn git(vault: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .arg("-C")
        .arg(vault)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap()
}
