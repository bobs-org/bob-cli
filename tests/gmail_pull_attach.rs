//! On-demand Gmail attachment import stays on the fake gog transport and
//! an isolated vault. Label filing lives in `gmail_sync.rs`.

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
