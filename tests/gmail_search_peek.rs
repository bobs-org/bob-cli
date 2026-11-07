//! Search, preview, and remote saved-thread counts for Bob Gmail.

mod gmail_support;

use std::{fs, os::unix::fs::PermissionsExt};

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

fn thread_get_calls(fake: &FakeGog) -> Vec<Vec<String>> {
    fake.calls()
        .into_iter()
        .filter(|call| {
            call.windows(3)
                .any(|items| items == ["gmail", "thread", "get"])
        })
        .collect()
}

fn saved_note(vault: &std::path::Path, messages: u64) {
    let path = vault.join("mail/2026/lease.md");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        format!(
            "---\ntype: \"[[mail]]\"\ncontent: untrusted-email\ngmail_account: you@example.com\ngmail_thread_id: 0123456789abcdef\ntitle: Lease renewal\nwho: Dana Whitfield\nstarted: 2026-11-05T12:00:00+00:00\nupdated: 2026-11-05T12:01:00+00:00\nmessages: {messages}\n---\nSaved mail.\n"
        ),
    )
    .unwrap();
}

#[test]
fn search_defaults_to_recent_inbox_caps_results_and_sanitizes_metadata() {
    let env = GmailEnv::new("bob-cli-gmail-search");
    let fake = FakeGog::new(&env, "gog").scenario("search_many");
    let output = run_gmail(&env, &fake, &["search", "-l", "99", "-f", "json"]);
    assert_success(&output, "gmail search");
    let document = json_stdout(&output);
    assert_eq!(document["query"], "in:inbox newer_than:30d");
    assert_eq!(document["limit"], 50);
    assert_eq!(document["threads"].as_array().unwrap().len(), 2);
    assert_eq!(document["threads"][0]["subject"], r"Lease renewal \#task");
    assert_eq!(document["threads"][0]["who"], "Dana Whitfield");
    assert_eq!(document["threads"][0]["messages"], 2);
    assert!(document["threads"][0]["snippet"].as_str().unwrap().len() <= 140);
    assert!(!document["threads"][0]["snippet"]
        .as_str()
        .unwrap()
        .contains("[["));
    assert_eq!(document["threads"][0]["saved_note"], Value::Null);

    let calls = fake.calls();
    let search = calls
        .iter()
        .find(|call| call.windows(2).any(|items| items == ["gmail", "search"]))
        .expect("Gmail search call");
    assert!(search.windows(2).any(|items| items == ["--max", "50"]));
    assert!(search.iter().any(|item| item == "in:inbox newer_than:30d"));
    let metadata = thread_get_calls(&fake);
    assert_eq!(metadata.len(), 2);
    // Real gog's `thread get` has no `--format`; omitting `--full` marks the
    // metadata read.
    assert!(metadata
        .iter()
        .all(|call| !call.iter().any(|item| item == "--format")));
    assert!(metadata
        .iter()
        .all(|call| !call.iter().any(|item| item == "--full")));
}

#[test]
fn search_uses_query_and_includes_saved_note_path() {
    let env = GmailEnv::new("bob-cli-gmail-search-saved");
    saved_note(env.vault(), 1);
    let fake = FakeGog::new(&env, "gog").scenario("search_one");
    let output = run_gmail(
        &env,
        &fake,
        &["search", "from:dana@example.test", "-f", "json"],
    );
    assert_success(&output, "gmail search saved");
    let document = json_stdout(&output);
    assert_eq!(document["query"], "from:dana@example.test");
    assert_eq!(document["limit"], 20);
    assert_eq!(document["threads"][0]["saved_note"], "mail/2026/lease.md");
    assert!(fake
        .calls()
        .iter()
        .any(|call| { call.windows(2).any(|items| items == ["--max", "20"]) }));
}

#[test]
fn peek_returns_preview_and_uses_private_body_free_cache_until_expiry() {
    let env = GmailEnv::new("bob-cli-gmail-peek");
    let fake = FakeGog::new(&env, "gog");
    let first =
        run_gmail(&env, &fake, &["peek", "0123456789abcdef", "-f", "json"]);
    assert_success(&first, "gmail peek");
    let document = json_stdout(&first);
    assert_eq!(document["verdict"], "resolved");
    assert_eq!(document["subject"], r"Lease renewal \#task");
    assert_eq!(document["who"], "Dana Whitfield");
    assert_eq!(document["messages"], 2);
    assert!(document["label"]
        .as_str()
        .unwrap()
        .contains("Dana Whitfield"));
    assert!(document["message"].as_str().unwrap().contains("2 messages"));

    let cache = env.root().join("home/.cache/bob-cli/gmail/peek.json");
    let cache_text = fs::read_to_string(&cache).expect("peek cache file");
    assert!(!cache_text.contains("snippet"));
    assert!(!cache_text.contains("payload"));
    assert!(!cache_text.contains("Hello"));
    assert_eq!(
        fs::metadata(&cache).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(cache.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );

    let second =
        run_gmail(&env, &fake, &["peek", "0123456789abcdef", "-f", "json"]);
    assert_success(&second, "cached gmail peek");
    assert_eq!(thread_get_calls(&fake).len(), 1);

    let mut cache_doc: Value = serde_json::from_str(&cache_text).unwrap();
    cache_doc["entries"]["you@example.com/0123456789abcdef"]["fetched_at"] =
        0.into();
    fs::write(&cache, serde_json::to_vec(&cache_doc).unwrap()).unwrap();
    let expired =
        run_gmail(&env, &fake, &["peek", "0123456789abcdef", "-f", "json"]);
    assert_success(&expired, "expired gmail peek");
    assert_eq!(thread_get_calls(&fake).len(), 2);
}

#[test]
fn peek_reports_unresolvable_links_without_fetching_thread_data() {
    let env = GmailEnv::new("bob-cli-gmail-peek-unresolvable");
    let fake = FakeGog::new(&env, "gog");
    let output =
        run_gmail(&env, &fake, &["peek", "not-a-gmail-thread", "-f", "json"]);
    assert_success(&output, "unresolvable gmail peek");
    let document = json_stdout(&output);
    assert_eq!(document["verdict"], "unresolvable");
    assert_eq!(document["thread_id"], Value::Null);
    assert!(!document["message"].as_str().unwrap().is_empty());
    assert!(thread_get_calls(&fake).is_empty());
}

#[test]
fn list_remote_shows_live_new_message_count() {
    let env = GmailEnv::new("bob-cli-gmail-list-remote");
    saved_note(env.vault(), 1);
    let fake = FakeGog::new(&env, "gog");
    let output = run_gmail(&env, &fake, &["list", "-r", "-f", "json"]);
    assert_success(&output, "gmail list remote");
    let document = json_stdout(&output);
    assert_eq!(document["threads"][0]["messages"], 1);
    assert_eq!(document["threads"][0]["new_messages"], 1);
    assert_eq!(thread_get_calls(&fake).len(), 1);
    assert!(!thread_get_calls(&fake)[0]
        .iter()
        .any(|item| item == "--format" || item == "--full"));
}
