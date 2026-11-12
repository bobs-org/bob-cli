//! Direct contract checks for the stateful, non-networking fake gog transport.

mod gmail_support;

use gmail_support::{json_stdout, stderr, FakeGog, GmailEnv};
use serde_json::{json, Value};

fn write_prefix<'a>(command: &'a str) -> Vec<&'a str> {
    vec![
        "--no-input",
        "--json",
        "--color",
        "never",
        "--client",
        "bob-gmail",
        "--account",
        "you@example.com",
        "--gmail-no-send",
        "--enable-commands-exact",
        command,
    ]
}

fn user_labels() -> Value {
    json!([
        {"id": "Label_42", "name": "Receipt, with\\slash", "type": "user"},
        {"id": "Label_1", "name": "Enrollment", "type": "user"},
        {"id": "INBOX", "name": "INBOX", "type": "system"}
    ])
}

fn seeded(fake: &FakeGog) {
    fake.set_labels(user_labels());
    fake.set_messages(json!({
        "aaaaaaaaaaaaaaaa": {"threadId": "1111111111111111", "labelIds": ["INBOX", "Label_1", "UNREAD"]},
        "bbbbbbbbbbbbbbbb": {"threadId": "1111111111111111", "labelIds": ["INBOX", "Label_1", "STARRED"]},
        "cccccccccccccccc": {"threadId": "2222222222222222", "labelIds": ["INBOX", "Label_1", "UNREAD"]}
    }));
}

#[test]
fn batch_modify_is_one_explicit_message_call_and_preserves_other_labels() {
    let env = GmailEnv::new("bob-cli-gmail-transport-batch");
    let fake = FakeGog::new(&env, "transport");
    seeded(&fake);

    let mut args = write_prefix("gmail.batch.modify");
    args.extend([
        "gmail",
        "batch",
        "modify",
        "aaaaaaaaaaaaaaaa",
        "bbbbbbbbbbbbbbbb",
        "--remove-label",
        "INBOX",
        "--add-label",
        "Label_42",
    ]);
    let output = fake.invoke(&args);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        json_stdout(&output),
        json!({
            "modified": ["aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb"],
            "count": 2,
            "addedLabels": ["Label_42"],
            "removedLabels": ["INBOX"]
        })
    );
    assert_eq!(
        fake.message_labels("aaaaaaaaaaaaaaaa"),
        ["Label_1", "Label_42", "UNREAD"]
    );
    assert_eq!(
        fake.message_labels("bbbbbbbbbbbbbbbb"),
        ["Label_1", "Label_42", "STARRED"]
    );
    assert_eq!(
        fake.message_labels("cccccccccccccccc"),
        ["INBOX", "Label_1", "UNREAD"]
    );

    let call = fake.calls().pop().expect("recorded batch call");
    assert!(call.contains(&"--gmail-no-send".to_string()));
    assert!(!call.contains(&"--readonly".to_string()));
    assert_eq!(
        call.windows(2)
            .find(|pair| pair[0] == "--enable-commands-exact")
            .unwrap()[1],
        "gmail.batch.modify"
    );
    assert!(call.contains(&"--add-label".to_string()));
    assert!(call.contains(&"--remove-label".to_string()));
}

#[test]
fn fake_injects_a_reply_between_preflight_and_the_frozen_batch() {
    let env = GmailEnv::new("bob-cli-gmail-transport-race");
    let fake = FakeGog::new(&env, "transport");
    seeded(&fake);
    fake.set_batch_race_before_modify(
        "cccccccccccccccc",
        &["INBOX", "Label_1", "Label_42", "UNREAD"],
    );

    let mut args = write_prefix("gmail.batch.modify");
    args.extend([
        "gmail",
        "batch",
        "modify",
        "aaaaaaaaaaaaaaaa",
        "--remove-label",
        "INBOX",
        "--add-label",
        "Label_42",
    ]);
    let output = fake.invoke(&args);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        fake.message_labels("aaaaaaaaaaaaaaaa"),
        ["Label_1", "Label_42", "UNREAD"]
    );
    assert_eq!(
        fake.message_labels("cccccccccccccccc"),
        ["INBOX", "Label_1", "Label_42", "UNREAD"],
        "the newly arrived reply is not in the frozen mutation list"
    );
}

#[test]
fn label_create_race_is_visible_after_the_error_and_literal_name_survives() {
    let env = GmailEnv::new("bob-cli-gmail-transport-label-race");
    let fake = FakeGog::new(&env, "transport");
    seeded(&fake);
    fake.set_labels(json!([
        {"id": "Label_1", "name": "Enrollment", "type": "user"},
        {"id": "INBOX", "name": "INBOX", "type": "system"}
    ]));
    fake.set_label_create_race("Receipt, with\\slash", "Label_99");

    let mut args = write_prefix("gmail.labels.create");
    args.extend(["gmail", "labels", "create", "Receipt, with\\slash"]);
    let output = fake.invoke(&args);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("alreadyExists"));

    let read_args = [
        "--no-input", "--json", "--color", "never", "--readonly", "--client",
        "bob-gmail", "--account", "you@example.com", "--gmail-no-send",
        "--enable-commands", "gmail.thread.get,gmail.get,gmail.messages,gmail.search,gmail.url,gmail.labels.list",
        "gmail", "labels", "list",
    ];
    let listed = fake.invoke(&read_args);
    assert!(listed.status.success(), "{}", stderr(&listed));
    let labels = json_stdout(&listed)["labels"].as_array().unwrap().clone();
    assert!(
        labels.iter().any(|label| {
            label["id"] == "Label_99" && label["name"] == "Receipt, with\\slash"
        }),
        "labels: {labels:?}"
    );

    let mut create = write_prefix("gmail.labels.create");
    create.extend(["gmail", "labels", "create", "Second, label\\name"]);
    let created = fake.invoke(&create);
    assert!(created.status.success(), "{}", stderr(&created));
    assert_eq!(
        json_stdout(&created)["label"]["name"],
        "Second, label\\name"
    );
}

#[test]
fn permission_and_partial_thread_failures_leave_only_successful_calls_applied()
{
    let env = GmailEnv::new("bob-cli-gmail-transport-failures");
    let fake = FakeGog::new(&env, "transport");
    seeded(&fake);
    fake.set_failure("batch_modify", "insufficient_scope: PRIVATE_SENTINEL", 1);

    let mut args = write_prefix("gmail.batch.modify");
    args.extend([
        "gmail",
        "batch",
        "modify",
        "aaaaaaaaaaaaaaaa",
        "--remove-label",
        "INBOX",
        "--add-label",
        "Label_42",
    ]);
    let denied = fake.invoke(&args);
    assert!(!denied.status.success());
    assert_eq!(
        fake.message_labels("aaaaaaaaaaaaaaaa"),
        ["INBOX", "Label_1", "UNREAD"]
    );

    fake.clear_failure("batch_modify");
    fake.set_fail_after(
        "batch_modify",
        "transient: second thread failed",
        1,
        1,
    );
    let first = fake.invoke(&args);
    assert!(first.status.success(), "{}", stderr(&first));
    let second_args = {
        let mut next = write_prefix("gmail.batch.modify");
        next.extend([
            "gmail",
            "batch",
            "modify",
            "cccccccccccccccc",
            "--remove-label",
            "INBOX",
            "--add-label",
            "Label_42",
        ]);
        next
    };
    let second = fake.invoke(&second_args);
    assert!(!second.status.success());
    assert_eq!(
        fake.message_labels("aaaaaaaaaaaaaaaa"),
        ["Label_1", "Label_42", "UNREAD"]
    );
    assert_eq!(
        fake.message_labels("cccccccccccccccc"),
        ["INBOX", "Label_1", "UNREAD"]
    );
}
