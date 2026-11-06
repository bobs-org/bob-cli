//! Bob Mail consumer contract: synthetic schema-1 preview and error fixtures
//! checked against live CLI output so `docs/gmail.md` and the plugin contract
//! cannot drift from the implementation.

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

fn assert_usage_error(output: &Output, label: &str) {
    assert_eq!(
        output.status.code(),
        Some(2),
        "{label} expected exit 2:\nstdout:\n{}\nstderr:\n{}",
        stdout(output),
        stderr(output)
    );
}

fn assert_fixture(output: &Output, label: &str, fixture: &str) {
    let live: Value = json_stdout(output);
    let pinned: Value =
        serde_json::from_str(fixture).expect("parse pinned fixture");
    assert_eq!(live, pinned, "{label} drifted from its pinned fixture");
}

fn author_rich_compose(env: &GmailEnv, fake: &FakeGog) -> String {
    let created = run_gmail(
        env,
        fake,
        &[
            "new",
            "-t",
            "ada@example.com",
            "-c",
            "boe@example.com",
            "-s",
            "Lease renewal",
            "-f",
            "json",
        ],
    );
    assert_success(&created, "create rich compose note");
    let note = json_stdout(&created)["note"]
        .as_str()
        .expect("note path")
        .to_string();
    fs::write(
        env.vault().join("report.pdf"),
        b"synthetic quarterly report bytes",
    )
    .expect("write synthetic attachment");
    let note_path = env.vault().join(&note);
    let authored = fs::read_to_string(&note_path)
        .expect("read fresh compose note")
        .replace("attachments: []", "attachments: [report.pdf]");
    fs::write(
        &note_path,
        format!(
            "{authored}Hello **Ada**, see [[lease|the lease]].\n\n![[report.pdf]]\n\n## Notes\n\nPrivate note.\n"
        ),
    )
    .expect("author rich compose body");
    note
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
    fs::create_dir_all(note_path.parent().unwrap()).expect("thread dir");
    fs::write(
        note_path,
        "---\nparent: \"[[mail]]\"\ntype: \"[[mail]]\"\ncontent: untrusted-email\ngmail_account: you@example.com\ngmail_thread_id: 0123456789abcdef\ntitle: Lease renewal\nwho: Dana\nstarted: 2026-11-05T10:00:00+00:00\nupdated: 2026-11-05T10:00:00+00:00\nmessages: 1\n---\n# Lease renewal\n\n· Dana ↔ you · 1 message · Nov 5\n\n## Notes\n\nKeep this private.\n\n## Conversation\n\n<!-- bob:gmail:begin v1 -->\n<!-- bob:gmail:msg 0123456789abcdef %3Clease%40example.test%3E -->\n> [!mail+] **Dana** · Nov 5\n> Hello\n^m-0123456789abcdef\n<!-- bob:gmail:end -->\n",
    )
    .expect("write saved thread note");
}

#[test]
fn send_preview_errors_and_sent_match_consumer_fixtures() {
    let env = GmailEnv::new("bob-cli-gmail-compose-contract");
    let fake = FakeGog::new(&env, "gog").scenario("compose_grant");
    let note = author_rich_compose(&env, &fake);

    let preview = run_gmail(&env, &fake, &["send", &note, "-d", "-f", "json"]);
    assert_success(&preview, "preview rich compose send");
    assert_fixture(
        &preview,
        "send preview",
        include_str!("fixtures/gmail_compose/send_preview.json"),
    );
    let digest = json_stdout(&preview)["digest"]
        .as_str()
        .expect("preview digest")
        .to_string();

    let required = run_gmail(&env, &fake, &["send", &note, "-f", "json"]);
    assert_usage_error(&required, "non-TTY send without confirmation");
    assert_fixture(
        &required,
        "confirmation_required",
        include_str!("fixtures/gmail_compose/error_confirmation_required.json"),
    );

    let mismatch = run_gmail(
        &env,
        &fake,
        &["send", &note, "-c", &"0".repeat(64), "-f", "json"],
    );
    assert_usage_error(&mismatch, "stale digest send");
    assert_fixture(
        &mismatch,
        "confirmation_mismatch",
        include_str!("fixtures/gmail_compose/error_confirmation_mismatch.json"),
    );

    let sent =
        run_gmail(&env, &fake, &["send", &note, "-c", &digest, "-f", "json"]);
    assert_success(&sent, "confirmed rich compose send");
    assert_fixture(
        &sent,
        "sent document",
        include_str!("fixtures/gmail_compose/send_sent.json"),
    );

    for call in fake.calls() {
        assert!(
            !call.windows(2).any(|pair| pair == ["gmail", "send"]),
            "never invoke direct gmail send: {call:?}"
        );
        assert!(
            !call.windows(2).any(|pair| pair == ["messages", "send"]),
            "never invoke messages send: {call:?}"
        );
    }
}

#[test]
fn render_refusal_matches_consumer_fixture() {
    let env = GmailEnv::new("bob-cli-gmail-compose-refusal");
    let fake = FakeGog::new(&env, "gog").scenario("compose_grant");
    let created = run_gmail(
        &env,
        &fake,
        &[
            "new",
            "-t",
            "cho@example.com",
            "-s",
            "Refusal sample",
            "-f",
            "json",
        ],
    );
    assert_success(&created, "create refusal compose note");
    let note = json_stdout(&created)["note"]
        .as_str()
        .expect("note path")
        .to_string();
    let note_path = env.vault().join(&note);
    let original = fs::read_to_string(&note_path).expect("read fresh note");
    fs::write(&note_path, format!("{original}- [ ] do the thing.\n"))
        .expect("author refused checkbox");

    let refused = run_gmail(&env, &fake, &["send", &note, "-d", "-f", "json"]);
    assert_usage_error(&refused, "refused checkbox send");
    assert_fixture(
        &refused,
        "render refusal",
        include_str!("fixtures/gmail_compose/error_render_refusal.json"),
    );
}

#[test]
fn reply_draft_preview_matches_consumer_fixture() {
    let env = GmailEnv::new("bob-cli-gmail-compose-reply");
    let fake = FakeGog::new(&env, "gog").scenario("compose_grant");
    let path = "mail/2026/261105_reply_thread.md";
    make_saved_thread(&env, &fake, path);
    let reply = run_gmail(&env, &fake, &["reply", path, "-f", "json"]);
    assert_success(&reply, "append reply section");
    let note_path = env.vault().join(path);
    let original = fs::read_to_string(&note_path).expect("read reply note");
    fs::write(
        &note_path,
        format!("{original}Thanks **Dana**, confirmed.\n"),
    )
    .expect("author reply body");

    let preview = run_gmail(&env, &fake, &["draft", path, "-f", "json"]);
    assert_success(&preview, "preview reply draft");
    assert_fixture(
        &preview,
        "reply draft preview",
        include_str!("fixtures/gmail_compose/draft_reply_preview.json"),
    );
}
