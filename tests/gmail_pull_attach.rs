//! Gmail label pull and on-demand attachment import stay on the fake gog
//! transport and an isolated vault.

mod gmail_support;

use std::fs;

use gmail_support::{
    json_stdout, run_gmail, stderr, stdout, FakeGog, GmailEnv,
};
use serde_json::Value;

fn assert_success(output: &std::process::Output, label: &str) {
    assert_eq!(
        output.status.code(),
        Some(0),
        "{label} expected exit 0:\nstdout:\n{}\nstderr:\n{}",
        stdout(output),
        stderr(output)
    );
}

#[test]
fn pull_creates_an_inbox_and_is_idempotent_by_thread_id() {
    let env = GmailEnv::new("bob-cli-gmail-pull");
    let fake = FakeGog::new(&env, "gog").scenario("gmail_pull_one");

    let first = run_gmail(&env, &fake, &["pull", "-f", "json"]);
    assert_success(&first, "first pull");
    let first_json = json_stdout(&first);
    assert_eq!(first_json["schema_version"], 1);
    assert_eq!(first_json["pulled"], 1);
    assert_eq!(first_json["results"][0]["status"], "filed");
    assert!(fake.called_with(&["label:bob"]));

    let inbox = fs::read_to_string(env.vault().join("gmail_inbox.md"))
        .expect("pull creates the configured inbox");
    assert!(inbox.contains("parent: \"[[inbox]]\""));
    assert!(inbox.contains("type: \"[[area]]\""));
    assert!(inbox.contains("ready_cap: off"));
    assert!(inbox.contains("## Tasks"));
    assert!(inbox.contains("- [ ] #task [[mail/"));
    assert!(inbox.contains("✉️ Pull sample · Ada"));
    assert!(!inbox.contains("[fresh::"));

    let second = run_gmail(&env, &fake, &["pull", "-f", "json"]);
    assert_success(&second, "second pull");
    let second_json = json_stdout(&second);
    assert_eq!(second_json["pulled"], 0);
    assert_eq!(second_json["results"][0]["status"], "already_pulled");
    let inbox_after = fs::read_to_string(env.vault().join("gmail_inbox.md"))
        .expect("inbox remains readable");
    assert_eq!(
        inbox_after.matches("- [ ] #task [[mail/").count(),
        1,
        "a repeated pull must not duplicate the task"
    );
}

#[test]
fn pull_dry_run_reads_but_writes_nothing() {
    let env = GmailEnv::new("bob-cli-gmail-pull-dry-run");
    let fake = FakeGog::new(&env, "gog").scenario("gmail_pull_one");

    let output = run_gmail(&env, &fake, &["pull", "-d", "-f", "json"]);
    assert_success(&output, "dry-run pull");
    let value = json_stdout(&output);
    assert_eq!(value["dry_run"], true);
    assert_eq!(value["results"][0]["status"], "would_file");
    assert!(!env.vault().join("gmail_inbox.md").exists());
    assert!(!env.vault().join("mail").exists());
    assert!(!env.vault().join(".bob/gmail").exists());
}

#[test]
fn pull_refuses_a_non_writer_and_a_disabled_label() {
    let env = GmailEnv::new("bob-cli-gmail-pull-writer");
    env.write_config(&format!(
        "gmail:\n  account: {}\n  writer: someone-else\n",
        gmail_support::ACCOUNT
    ));
    let fake = FakeGog::new(&env, "gog").scenario("gmail_pull_one");
    let writer = run_gmail(&env, &fake, &["pull", "-f", "json"]);
    assert_eq!(writer.status.code(), Some(2));
    let writer_json: Value = serde_json::from_slice(&writer.stdout).unwrap();
    assert_eq!(writer_json["error"]["kind"], "writer");
    assert!(!fake.called_with(&["gmail", "search"]));

    env.write_config(&format!(
        "gmail:\n  account: {}\n  pull:\n    label: ''\n",
        gmail_support::ACCOUNT
    ));
    let disabled = run_gmail(&env, &fake, &["pull", "-f", "json"]);
    assert_eq!(disabled.status.code(), Some(2));
    let value: Value = serde_json::from_slice(&disabled.stdout).unwrap();
    assert_eq!(value["error"]["kind"], "pull_disabled");
}

#[test]
fn attachment_dry_run_is_read_only_and_import_is_idempotent() {
    let env = GmailEnv::new("bob-cli-gmail-attach");
    let fake = FakeGog::new(&env, "gog").scenario("gmail_attachment");
    let saved =
        run_gmail(&env, &fake, &["save", "0123456789abcdef", "-f", "json"]);
    assert_success(&saved, "save attachment fixture");
    let note = json_stdout(&saved)["note"]
        .as_str()
        .expect("saved note path")
        .to_string();
    let note_abs = env.vault().join(&note);
    let before = fs::read_to_string(&note_abs).expect("saved note exists");
    assert!(before.contains("📎 1. offer.pdf · 5 B"));

    let dry =
        run_gmail(&env, &fake, &["attach", &note, "1", "-d", "-f", "json"]);
    assert_success(&dry, "dry-run attach");
    let dry_json = json_stdout(&dry);
    assert_eq!(dry_json["status"], "would_import");
    assert!(!env.vault().join("file/mail").exists());
    assert_eq!(fs::read_to_string(&note_abs).unwrap(), before);

    let imported =
        run_gmail(&env, &fake, &["attach", &note, "1", "-f", "json"]);
    assert_success(&imported, "attach");
    let imported_json = json_stdout(&imported);
    assert_eq!(imported_json["status"], "imported");
    let path = imported_json["path"].as_str().unwrap();
    assert_eq!(fs::read(env.vault().join(path)).unwrap(), b"hello");
    let updated = fs::read_to_string(&note_abs).unwrap();
    assert!(updated.contains(&format!("📎 [[{path}|offer.pdf]] · 5 B")));
    assert!(updated.matches("📎 [[file/mail/").count() == 1);

    let api_calls_before = fake
        .calls()
        .iter()
        .filter(|call| call.windows(2).any(|pair| pair == ["api", "call"]))
        .count();
    let repeated =
        run_gmail(&env, &fake, &["attach", &note, "1", "-f", "json"]);
    assert_success(&repeated, "repeat attach");
    assert_eq!(json_stdout(&repeated)["status"], "unchanged");
    let api_calls_after = fake
        .calls()
        .iter()
        .filter(|call| call.windows(2).any(|pair| pair == ["api", "call"]))
        .count();
    assert_eq!(api_calls_after, api_calls_before);
    let journal = fs::read_to_string(
        env.vault().join(".bob/attachments/test-host.jsonl"),
    )
    .expect("attachment origin journal");
    assert_eq!(journal.matches("\"via\":\"gmail\"").count(), 1);
}

#[test]
fn oversized_attachment_is_rejected_before_download() {
    let env = GmailEnv::new("bob-cli-gmail-attach-large");
    let fake = FakeGog::new(&env, "gog").scenario("gmail_attachment_large");
    let saved =
        run_gmail(&env, &fake, &["save", "0123456789abcdef", "-f", "json"]);
    assert_success(&saved, "save large attachment fixture");
    let note = json_stdout(&saved)["note"].as_str().unwrap().to_string();
    let output = run_gmail(&env, &fake, &["attach", &note, "1", "-f", "json"]);
    assert_eq!(output.status.code(), Some(1));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["error"]["kind"], "too_large");
    assert!(!fake
        .calls()
        .iter()
        .any(|call| call.windows(2).any(|pair| pair == ["api", "call"])));
    assert!(!env.vault().join("file/mail").exists());
}
