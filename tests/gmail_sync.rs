//! `bob gmail sync` (and its hidden `pull` alias) stays on the fake gog
//! transport and an isolated vault.

mod gmail_support;

use std::fs;

use gmail_support::{
    json_stdout, run_gmail, stderr, stdout, FakeGog, GmailEnv, ACCOUNT, HOST,
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

fn write_area(env: &GmailEnv, route: &str) {
    fs::write(
        env.vault().join(format!("{route}.md")),
        format!("---\ntype: \"[[area]]\"\n---\n\n# {route}\n\n## Tasks\n"),
    )
    .expect("write area note");
}

#[test]
fn sync_creates_an_inbox_and_is_idempotent_by_thread_id() {
    let env = GmailEnv::new("bob-cli-gmail-sync");
    let fake = FakeGog::new(&env, "gog").scenario("gmail_pull_one");

    let first = run_gmail(&env, &fake, &["sync", "-f", "json"]);
    assert_success(&first, "first sync");
    let first_json = json_stdout(&first);
    assert_eq!(first_json["schema_version"], 1);
    assert_eq!(first_json["ok"], true);
    assert_eq!(first_json["pulled"], 1);
    assert_eq!(first_json["results"][0]["status"], "filed");
    assert_eq!(first_json["results"][0]["task_file"], "gmail_inbox.md");
    assert!(first_json["results"][0]["route"].is_null());
    assert!(first_json["watch"].as_array().unwrap().is_empty());
    assert!(first_json["problems"].as_array().unwrap().is_empty());
    assert_eq!(first_json["summary"]["watching"], 1);
    assert_eq!(first_json["summary"]["awaiting"], 0);
    assert!(fake.called_with(&["label:bob"]));

    let inbox = fs::read_to_string(env.vault().join("gmail_inbox.md"))
        .expect("sync creates the configured inbox");
    assert!(inbox.contains("parent: \"[[inbox]]\""));
    assert!(inbox.contains("type: \"[[area]]\""));
    assert!(inbox.contains("ready_cap: off"));
    assert!(inbox.contains("## Tasks"));
    assert!(inbox.contains("- [ ] #task [[mail/"));
    assert!(inbox.contains("✉️ Pull sample · Ada"));
    assert!(!inbox.contains("[fresh::"));

    let second = run_gmail(&env, &fake, &["sync", "-f", "json"]);
    assert_success(&second, "second sync");
    let second_json = json_stdout(&second);
    assert_eq!(second_json["pulled"], 0);
    assert_eq!(second_json["results"][0]["status"], "already_filed");
    assert_eq!(second_json["summary"]["watching"], 1);
    let inbox_after = fs::read_to_string(env.vault().join("gmail_inbox.md"))
        .expect("inbox remains readable");
    assert_eq!(
        inbox_after.matches("- [ ] #task [[mail/").count(),
        1,
        "a repeated sync must not duplicate the task"
    );

    let human = run_gmail(&env, &fake, &["sync"]);
    assert_success(&human, "human nothing new");
    let text = stdout(&human);
    assert!(text.contains("gmail sync"), "{text}");
    assert!(text.contains("nothing new"), "{text}");
    assert!(!text.contains("filed"), "{text}");
}

#[test]
fn sync_dry_run_reads_but_writes_nothing() {
    let env = GmailEnv::new("bob-cli-gmail-sync-dry-run");
    let fake = FakeGog::new(&env, "gog").scenario("gmail_pull_one");

    let output = run_gmail(&env, &fake, &["sync", "-d", "-f", "json"]);
    assert_success(&output, "dry-run sync");
    let value = json_stdout(&output);
    assert_eq!(value["dry_run"], true);
    assert_eq!(value["results"][0]["status"], "would_file");
    assert_eq!(value["summary"]["watching"], 1);
    assert!(!env.vault().join("gmail_inbox.md").exists());
    assert!(!env.vault().join("mail").exists());
    assert!(!env.vault().join(".bob/gmail").exists());
}

#[test]
fn hidden_pull_alias_matches_sync_json() {
    let env = GmailEnv::new("bob-cli-gmail-sync-alias");
    let fake = FakeGog::new(&env, "gog").scenario("gmail_pull_one");
    let sync = run_gmail(&env, &fake, &["sync", "-d", "-f", "json"]);
    assert_success(&sync, "sync dry-run");
    let pull = run_gmail(&env, &fake, &["pull", "-d", "-f", "json"]);
    assert_success(&pull, "pull dry-run alias");
    assert_eq!(json_stdout(&sync), json_stdout(&pull));
}

#[test]
fn sync_refuses_a_non_writer_and_a_disabled_label() {
    let env = GmailEnv::new("bob-cli-gmail-sync-writer");
    env.write_config(&format!(
        "gmail:\n  account: {ACCOUNT}\n  writer: someone-else\n"
    ));
    let fake = FakeGog::new(&env, "gog").scenario("gmail_pull_one");
    let writer = run_gmail(&env, &fake, &["sync", "-f", "json"]);
    assert_eq!(writer.status.code(), Some(2));
    let writer_json: Value = serde_json::from_slice(&writer.stdout).unwrap();
    assert_eq!(writer_json["error"]["kind"], "writer");
    assert!(!fake.called_with(&["gmail", "search"]));

    env.write_config(&format!(
        "gmail:\n  account: {ACCOUNT}\n  writer: {HOST}\n  pull:\n    label: ''\n"
    ));
    let disabled = run_gmail(&env, &fake, &["sync", "-f", "json"]);
    assert_eq!(disabled.status.code(), Some(2));
    let value: Value = serde_json::from_slice(&disabled.stdout).unwrap();
    assert_eq!(value["error"]["kind"], "sync_disabled");
}

#[test]
fn sync_routes_to_the_matching_area_note() {
    let env = GmailEnv::new("bob-cli-gmail-sync-route");
    write_area(&env, "job");
    let fake = FakeGog::new(&env, "gog").scenario("gmail_pull_one");
    fake.set_labels(serde_json::json!([
        {"id": "Label_1", "name": "job", "type": "user"},
        {"id": "INBOX", "name": "INBOX", "type": "system"},
    ]));

    let output = run_gmail(&env, &fake, &["sync", "-f", "json"]);
    assert_success(&output, "routed sync");
    let value = json_stdout(&output);
    assert_eq!(value["results"][0]["status"], "filed");
    assert_eq!(value["results"][0]["task_file"], "job.md");
    assert_eq!(value["results"][0]["route"], "job");
    assert!(!env.vault().join("gmail_inbox.md").exists());
    let job = fs::read_to_string(env.vault().join("job.md")).expect("job note");
    assert!(job.contains("- [ ] #task [[mail/"), "{job}");
}

#[test]
fn first_seen_route_problem_exits_1_and_files_the_inbox() {
    let env = GmailEnv::new("bob-cli-gmail-sync-problem");
    env.write_config(&format!(
        "gmail:\n  account: {ACCOUNT}\n  writer: {HOST}\n  sync:\n    route_prefixes: [\"#\"]\n"
    ));
    let fake = FakeGog::new(&env, "gog").scenario("gmail_pull_one");
    fake.set_labels(serde_json::json!([
        {"id": "Label_1", "name": "#/missing", "type": "user"},
        {"id": "INBOX", "name": "INBOX", "type": "system"},
    ]));

    let output = run_gmail(&env, &fake, &["sync", "-f", "json"]);
    assert_eq!(output.status.code(), Some(1), "{}", stdout(&output));
    let value = json_stdout(&output);
    assert_eq!(value["ok"], false);
    assert_eq!(value["pulled"], 1);
    assert_eq!(value["results"][0]["status"], "problem");
    assert_eq!(value["results"][0]["task_file"], "gmail_inbox.md");
    assert_eq!(value["problems"][0]["first_seen"], true);
    assert_eq!(value["problems"][0]["kind"], "unknown_note");
    let inbox = fs::read_to_string(env.vault().join("gmail_inbox.md"))
        .expect("problem files the inbox");
    assert!(inbox.contains("GMAIL ROUTE"), "{inbox}");
}

#[test]
fn busy_guard_is_a_quiet_success_skip() {
    use fs2::FileExt;
    let env = GmailEnv::new("bob-cli-gmail-sync-busy");
    let fake = FakeGog::new(&env, "gog").scenario("gmail_pull_one");
    let lock_path = env.root().join("state/bob/gmail").join(
        "sync-53e6cdc30765aade0129f85e5aeb50124b1d3f5bb9a70373be31e4eb328371e0.lock",
    );
    fs::create_dir_all(lock_path.parent().unwrap()).expect("guard dir");
    let guard = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .expect("open guard");
    guard.try_lock_exclusive().expect("hold sync guard");

    let json = run_gmail(&env, &fake, &["sync", "-f", "json"]);
    assert_success(&json, "busy json skip");
    let value = json_stdout(&json);
    assert_eq!(value["ok"], true);
    assert_eq!(value["pulled"], 0);
    assert!(value["results"].as_array().unwrap().is_empty());

    let human = run_gmail(&env, &fake, &["sync", "-q"]);
    assert_success(&human, "busy quiet skip");
    assert!(stdout(&human).is_empty(), "{}", stdout(&human));
    assert!(!env.vault().join("gmail_inbox.md").exists());
    drop(guard);
}

#[test]
fn help_lists_sync_and_hides_the_pull_alias() {
    let env = GmailEnv::new("bob-cli-gmail-sync-help");
    let fake = FakeGog::new(&env, "gog");
    let top = run_gmail(&env, &fake, &["--help"]);
    assert_success(&top, "gmail help");
    let help = stdout(&top);
    assert!(help.contains("sync"), "{help}");
    assert!(
        !help
            .lines()
            .any(|line| line.trim_start().starts_with("pull")),
        "hidden pull alias leaked:\n{help}"
    );

    let sync_help = run_gmail(&env, &fake, &["sync", "--help"]);
    assert_success(&sync_help, "sync help");
    let sync_text = stdout(&sync_help);
    assert!(sync_text.contains("-r, --retry-timeout"), "{sync_text}");
    assert!(sync_text.contains("-l, --limit"), "{sync_text}");
    let options: Vec<&str> = [
        "-b, --bob-dir",
        "-d, --dry-run",
        "-f, --format",
        "-h, --help",
        "-l, --limit",
        "-q, --quiet",
        "-r, --retry-timeout",
    ]
    .into_iter()
    .collect();
    let mut last = 0usize;
    for option in &options {
        let position = sync_text.find(option).unwrap_or_else(|| {
            panic!("missing {option} in sync help:\n{sync_text}")
        });
        assert!(
            position >= last,
            "{option} is out of order in:\n{sync_text}"
        );
        last = position;
    }

    let pull_help = run_gmail(&env, &fake, &["pull", "--help"]);
    assert_success(&pull_help, "pull alias help");
    assert!(
        stdout(&pull_help).contains("bob gmail sync"),
        "{}",
        stdout(&pull_help)
    );
}
