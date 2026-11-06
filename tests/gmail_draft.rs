//! Gmail draft create/update/readback acceptance tests against fake gog.

mod gmail_support;

use std::{fs, process::Output};

use gmail_support::{
    json_stdout, run_gmail, stderr, stdout, FakeGog, GmailEnv,
};
use serde_json::Value;

fn assert_success(output: &Output, label: &str) {
    assert_eq!(
        output.status.code(),
        Some(0),
        "{label} expected exit 0:\nstdout:\n{}\nstderr:\n{}",
        stdout(output),
        stderr(output)
    );
}

fn draft_calls(fake: &FakeGog, operation: &str) -> Vec<Vec<String>> {
    fake.calls()
        .into_iter()
        .filter(|call| {
            call.windows(3)
                .any(|items| items == ["gmail", "drafts", operation])
        })
        .collect()
}

fn make_compose_note(env: &GmailEnv, fake: &FakeGog, subject: &str) -> String {
    let created = run_gmail(
        env,
        fake,
        &["new", "-t", "ada@example.com", "-s", subject, "-f", "json"],
    );
    assert_success(&created, "create local compose note");
    json_stdout(&created)["note"]
        .as_str()
        .expect("compose note path")
        .to_string()
}

#[test]
fn draft_create_readback_and_update_keep_one_remote_draft() {
    let env = GmailEnv::new("bob-cli-gmail-draft-roundtrip");
    let fake = FakeGog::new(&env, "gog").scenario("compose_grant");
    let note = make_compose_note(&env, &fake, "Lease renewal");
    let note_path = env.vault().join(&note);
    let original = fs::read_to_string(&note_path).unwrap();
    fs::write(&note_path, format!("{original}Hello **Ada**.\n")).unwrap();

    let first = run_gmail(&env, &fake, &["draft", &note, "-f", "json"]);
    assert_success(&first, "create Gmail draft");
    let first_doc = json_stdout(&first);
    assert_eq!(first_doc["status"], "preview");
    assert_eq!(
        first_doc["preview"]["to"].as_array().unwrap()[0],
        "ada@example.com"
    );
    assert_eq!(first_doc["preview"]["subject"], "Lease renewal");
    assert_eq!(
        first_doc["preview"]["text_body"],
        "Hello Ada.\n\n-- \nTest signature"
    );
    assert_eq!(first_doc["preview"]["signature"], "gmail");
    let draft_id = first_doc["draft_id"].as_str().unwrap().to_string();
    let digest = first_doc["digest"].as_str().unwrap().to_string();

    let updated_note = fs::read_to_string(&note_path).unwrap();
    assert!(updated_note.contains(&format!("gmail_draft_id: {draft_id}")));
    assert!(updated_note.contains("gmail_draft_account: you@example.com"));
    assert!(updated_note.contains("gmail_draft_remote_fingerprint:"));
    assert!(updated_note.ends_with("Hello **Ada**.\n"));

    let calls = fake.calls();
    let create = draft_calls(&fake, "create");
    let get = draft_calls(&fake, "get");
    assert_eq!(create.len(), 1, "one remote draft create: {calls:?}");
    assert_eq!(get.len(), 1, "read back the created draft: {calls:?}");
    assert!(create[0].contains(&"--gmail-no-send".to_string()));
    assert!(create[0].contains(&"gmail.drafts.create".to_string()));
    assert!(!create[0].contains(&"--readonly".to_string()));
    assert!(get[0].contains(&"--readonly".to_string()));
    assert!(!get[0].contains(&"--download".to_string()));
    assert!(create[0].iter().all(|arg| !arg.contains("Hello Ada")));

    fs::write(&note_path, format!("{updated_note}\nSecond paragraph.\n"))
        .unwrap();
    let second = run_gmail(&env, &fake, &["draft", &note, "-f", "json"]);
    assert_success(&second, "update Gmail draft");
    let second_doc = json_stdout(&second);
    assert_eq!(second_doc["draft_id"], draft_id);
    assert_ne!(second_doc["digest"], digest);
    assert_eq!(
        second_doc["preview"]["text_body"],
        "Hello Ada.\n\nSecond paragraph.\n\n-- \nTest signature"
    );
    assert_eq!(draft_calls(&fake, "create").len(), 1);
    assert_eq!(draft_calls(&fake, "update").len(), 1);
    assert_eq!(draft_calls(&fake, "get").len(), 3);

    let durable_state = fs::read_to_string(&note_path).unwrap();
    assert!(durable_state.contains(&format!("gmail_draft_id: {draft_id}")));
    let bob_state = env.vault().join(".bob/gmail");
    if bob_state.exists() {
        for entry in fs::read_dir(bob_state).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                let text = fs::read_to_string(path).unwrap_or_default();
                assert!(!text.contains("Second paragraph."));
            }
        }
    }
}

#[test]
fn draft_requires_compose_scope_before_creating_remote_state() {
    let env = GmailEnv::new("bob-cli-gmail-draft-scope");
    let fake = FakeGog::new(&env, "gog");
    let note = make_compose_note(&env, &fake, "Scope check");
    let note_path = env.vault().join(&note);
    let original = fs::read_to_string(&note_path).unwrap();
    fs::write(&note_path, format!("{original}Body stays local.\n")).unwrap();
    let before = fs::read_to_string(&note_path).unwrap();

    let output = run_gmail(&env, &fake, &["draft", &note, "-f", "json"]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    let error: Value = json_stdout(&output);
    assert_eq!(error["error"]["kind"], "insufficient_scope");
    assert!(draft_calls(&fake, "create").is_empty());
    assert!(draft_calls(&fake, "update").is_empty());
    assert_eq!(fs::read_to_string(&note_path).unwrap(), before);
}

#[test]
fn reply_all_uses_the_exact_saved_message_and_verifies_threading() {
    let env = GmailEnv::new("bob-cli-gmail-draft-reply-all");
    let fake = FakeGog::new(&env, "gog").scenario("compose_grant");
    let note = "mail/2026/lease.md";
    let note_path = env.vault().join(note);
    fs::create_dir_all(note_path.parent().unwrap()).unwrap();
    fs::write(
        &note_path,
        "---\ntype: \"[[mail]]\"\ncontent: untrusted-email\ngmail_account: you@example.com\ngmail_thread_id: 0123456789abcdef\ntitle: Lease\nwho: Dana\nstarted: 2026-11-05T10:00:00+00:00\nupdated: 2026-11-05T10:00:00+00:00\nmessages: 1\n---\n# Lease\n\n· Dana ↔ you · 1 message · Nov 5\n\n## Notes\n\nPrivate note\n\n## Conversation\n\n<!-- bob:gmail:begin v1 -->\n<!-- bob:gmail:msg 0123456789abcdef %3Clease%40example.test%3E -->\n> [!mail+] **Dana** · Nov 5\n> Hello\n^m-0123456789abcdef\n<!-- bob:gmail:end -->\n",
    )
    .unwrap();
    fake.set_thread(
        serde_json::json!({
            "thread": {
                "id": "0123456789abcdef",
                "messages": [{
                    "id": "0123456789abcdef",
                    "threadId": "0123456789abcdef",
                    "labelIds": ["INBOX"],
                    "payload": {
                        "mimeType": "text/plain",
                        "headers": [
                            {"name": "From", "value": "Ada <ada@example.com>"},
                            {"name": "To", "value": "you@example.com"},
                            {"name": "Cc", "value": "Lee <lee@example.com>"},
                            {"name": "Subject", "value": "Lease renewal"},
                            {"name": "Message-ID", "value": "<lease@example.test>"}
                        ],
                        "body": {"size": 5, "data": "SGVsbG8="}
                    }
                }]
            }
        }),
        serde_json::json!({}),
    );

    let appended =
        run_gmail(&env, &fake, &["reply", note, "--all", "-f", "json"]);
    assert_success(&appended, "append reply-all section");
    fs::write(
        &note_path,
        format!(
            "{}Replying to everyone.\n",
            fs::read_to_string(&note_path).unwrap()
        ),
    )
    .unwrap();

    let output = run_gmail(&env, &fake, &["draft", note, "-f", "json"]);
    assert_success(&output, "create reply-all draft");
    let doc = json_stdout(&output);
    assert_eq!(doc["preview"]["subject"], "Re: Lease renewal");
    assert_eq!(
        doc["preview"]["reply_target"]["thread_id"],
        "0123456789abcdef"
    );
    assert_eq!(
        doc["preview"]["reply_target"]["message_id"],
        "0123456789abcdef"
    );
    assert_eq!(doc["preview"]["reply_target"]["mode"], "reply_all");
    assert_eq!(doc["preview"]["to"][0], "Ada <ada@example.com>");
    assert_eq!(doc["preview"]["cc"][0], "Lee <lee@example.com>");
    let create = draft_calls(&fake, "create");
    assert_eq!(create.len(), 1);
    assert!(create[0]
        .windows(2)
        .any(|pair| { pair == ["--reply-to-message-id", "0123456789abcdef"] }));
    assert!(create[0].contains(&"--reply-all".to_string()));
    assert!(!create[0].contains(&"--thread-id".to_string()));
    assert!(!create[0]
        .iter()
        .any(|arg| arg.contains("Replying to everyone")));
    let stored = fs::read_to_string(note_path).unwrap();
    assert!(stored.contains("<!-- bob:gmail:draft draft-1 -->"));
    assert!(stored.contains("gmail_draft_remote_fingerprint:"));
}

#[test]
fn draft_attachments_are_snapshotted_and_removed_explicitly() {
    let env = GmailEnv::new("bob-cli-gmail-draft-attachments");
    let fake = FakeGog::new(&env, "gog").scenario("compose_grant");
    let note = make_compose_note(&env, &fake, "Attachment update");
    let note_path = env.vault().join(&note);
    let original = fs::read_to_string(&note_path).unwrap();
    fs::write(
        &note_path,
        original.replace("attachments: []", "attachments: [brief.txt]"),
    )
    .unwrap();
    fs::write(env.vault().join("brief.txt"), b"private attachment bytes")
        .unwrap();
    let with_attachment = fs::read_to_string(&note_path).unwrap();
    fs::write(
        &note_path,
        format!("{with_attachment}Please see the file.\n"),
    )
    .unwrap();

    let first = run_gmail(&env, &fake, &["draft", &note, "-f", "json"]);
    assert_success(&first, "create draft with attachment");
    let first_doc = json_stdout(&first);
    assert_eq!(
        first_doc["preview"]["attachments"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        first_doc["preview"]["attachments"][0]["filename"],
        "brief.txt"
    );
    let create = draft_calls(&fake, "create");
    assert!(create[0].windows(2).any(|pair| pair[0] == "--attach"));
    assert!(create[0]
        .iter()
        .all(|arg| !arg.contains("private attachment bytes")));

    let updated = fs::read_to_string(&note_path).unwrap();
    fs::write(
        &note_path,
        updated.replace("attachments: [brief.txt]", "attachments: []"),
    )
    .unwrap();
    let second = run_gmail(&env, &fake, &["draft", &note, "-f", "json"]);
    assert_success(&second, "remove draft attachment");
    assert!(json_stdout(&second)["preview"]["attachments"]
        .as_array()
        .unwrap()
        .is_empty());
    let update = draft_calls(&fake, "update");
    assert_eq!(update.len(), 1);
    assert!(update[0].contains(&"--clear-attachments".to_string()));
}

#[test]
fn unknown_create_result_retains_marker_and_never_retries_create() {
    let env = GmailEnv::new("bob-cli-gmail-draft-unknown-create");
    let fake = FakeGog::new(&env, "gog").scenario("compose_grant");
    let note = make_compose_note(&env, &fake, "Unknown create result");
    let note_path = env.vault().join(&note);
    let original = fs::read_to_string(&note_path).unwrap();
    fs::write(&note_path, format!("{original}Private body sentinel.\n"))
        .unwrap();
    fake.set_failure("draft_create", "PRIVATE_SENTINEL remote error", 1);

    let first = run_gmail(&env, &fake, &["draft", &note, "-f", "json"]);
    assert_eq!(first.status.code(), Some(1), "{}", stderr(&first));
    let first_error = json_stdout(&first);
    assert_eq!(first_error["error"]["kind"], "draft_create_unknown");
    assert!(!stdout(&first).contains("PRIVATE_SENTINEL"));
    assert!(!stderr(&first).contains("PRIVATE_SENTINEL"));
    let marked = fs::read_to_string(&note_path).unwrap();
    assert!(marked.contains("gmail_draft_create_operation:"));
    assert!(marked.contains("Private body sentinel."));
    assert!(!marked.contains("PRIVATE_SENTINEL"));

    let second = run_gmail(&env, &fake, &["draft", &note, "-f", "json"]);
    assert_eq!(second.status.code(), Some(1), "{}", stderr(&second));
    assert_eq!(
        json_stdout(&second)["error"]["kind"],
        "draft_create_unresolved"
    );
    assert_eq!(draft_calls(&fake, "create").len(), 1);
}
