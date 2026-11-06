//! Confirmed Gmail send and in-place sent-note finalization against fake gog.

mod gmail_support;

use std::{fs, process::Output};

use gmail_support::{
    json_stdout, run_gmail, run_gmail_stdin, stderr, stdout, FakeGog, GmailEnv,
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
