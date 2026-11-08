//! `bob gmail sync` (and its hidden `pull` alias) stays on the fake gog
//! transport and an isolated vault.

mod gmail_support;

use std::{fs, path::Path, process::Command};

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

fn threads_list_queries(fake: &FakeGog) -> Vec<String> {
    fake.calls()
        .iter()
        .filter(|call| call.iter().any(|arg| arg == "gmail.users.threads.list"))
        .filter_map(|call| {
            call.windows(2)
                .find(|pair| pair[0] == "--params")
                .map(|pair| {
                    let params: Value = serde_json::from_str(&pair[1])
                        .expect("threads.list --params is JSON");
                    params["q"].as_str().unwrap_or_default().to_string()
                })
        })
        .collect()
}

fn threads_list_page_tokens(fake: &FakeGog) -> Vec<Option<String>> {
    fake.calls()
        .iter()
        .filter(|call| call.iter().any(|arg| arg == "gmail.users.threads.list"))
        .filter_map(|call| {
            call.windows(2)
                .find(|pair| pair[0] == "--params")
                .map(|pair| {
                    let params: Value = serde_json::from_str(&pair[1])
                        .expect("threads.list --params is JSON");
                    params
                        .get("pageToken")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
        })
        .collect()
}

fn seed_unenrolled_task(
    env: &GmailEnv,
    fake: &FakeGog,
    thread_id: &str,
    task_file: &str,
) -> (String, String) {
    let saved = run_gmail(env, fake, &["save", thread_id, "-f", "json"]);
    assert_success(&saved, "save unenrolled mail note");
    let doc = json_stdout(&saved);
    let note = doc["note"].as_str().expect("saved note path").to_string();
    let link = doc["link"].as_str().expect("saved note link").to_string();

    let path = env.vault().join(task_file);
    if !path.exists() {
        let area = task_file.strip_suffix(".md").unwrap_or(task_file);
        write_area(env, area);
    }
    let mut text = fs::read_to_string(&path).expect("task note");
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&format!("- [ ] #task Existing {link}\n"));
    fs::write(path, text).expect("write existing task");
    (note, link)
}

fn journal_kinds(env: &GmailEnv) -> Vec<String> {
    let dir = env.vault().join(".bob/gmail");
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| fs::read_to_string(entry.path()).ok())
        .flat_map(|text| {
            text.lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .filter_map(|event| event["kind"].as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .collect()
}

fn task_row_count(path: &Path) -> usize {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|line| line.contains("#task") && line.contains("[[mail/"))
        .count()
}

fn snapshot_tree(root: &Path, skip_git: bool) -> Vec<(String, Vec<u8>)> {
    fn visit(
        root: &Path,
        current: &Path,
        skip_git: bool,
        out: &mut Vec<(String, Vec<u8>)>,
    ) {
        let Ok(entries) = fs::read_dir(current) else {
            return;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let relative =
                path.strip_prefix(root).expect("relative fixture path");
            if skip_git && relative == Path::new(".git") {
                continue;
            }
            let name = relative.to_string_lossy().replace('\\', "/");
            if path.is_dir() {
                out.push((name, Vec::new()));
                visit(root, &path, skip_git, out);
            } else if let Ok(bytes) = fs::read(&path) {
                out.push((name, bytes));
            }
        }
    }

    let mut out = Vec::new();
    if root.is_dir() {
        visit(root, root, skip_git, &mut out);
    }
    out.sort_by(|left, right| left.0.cmp(&right.0));
    out
}

fn git(vault: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .arg("-C")
        .arg(vault)
        .args(args)
        .output()
        .expect("run git")
}

fn initialize_git_vault(vault: &Path) {
    let output = git(vault, &["init", "-q"]);
    assert!(
        output.status.success(),
        "git init: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    for (key, value) in [
        ("user.name", "Gmail Sync Tests"),
        ("user.email", "gmail-sync-tests@example.test"),
    ] {
        let output = git(vault, &["config", "--local", key, value]);
        assert!(output.status.success(), "git config {key}");
    }
    fs::write(vault.join("baseline.md"), "baseline\n").expect("write baseline");
    assert!(git(vault, &["add", "baseline.md"]).status.success());
    assert!(git(vault, &["commit", "-qm", "baseline"]).status.success());
}

fn git_subjects(vault: &Path) -> Vec<String> {
    let output = git(vault, &["log", "--format=%s"]);
    assert!(
        output.status.success(),
        "git log: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("git subjects are UTF-8")
        .lines()
        .map(str::to_string)
        .collect()
}

fn normalize_root(bytes: &[u8], root: &Path) -> Vec<u8> {
    let needle = root.as_os_str().to_string_lossy().as_bytes().to_vec();
    if needle.is_empty() {
        return bytes.to_vec();
    }
    let mut out = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at..].starts_with(&needle) {
            out.extend_from_slice(b"<ROOT>");
            at += needle.len();
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    out
}

#[derive(Debug, PartialEq, Eq)]
struct AliasSnapshot {
    status: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    vault: Vec<(String, Vec<u8>)>,
    state: Vec<(String, Vec<u8>)>,
    git_subjects: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
enum AliasScenario {
    LiveSuccess,
    DryRunHuman,
    DryRunJson,
    RouteProblem,
    Help,
    SetupFailure,
}

fn run_alias_fixture(alias: &str, scenario: AliasScenario) -> AliasSnapshot {
    let env = GmailEnv::new("bob-cli-gmail-alias-parity");
    initialize_git_vault(env.vault());
    let fake = FakeGog::new(&env, "gog").scenario("gmail_pull_one");
    let mut args: Vec<&str> = vec![alias];
    match scenario {
        AliasScenario::LiveSuccess => args.extend(["-f", "json"]),
        AliasScenario::DryRunHuman => args.push("-d"),
        AliasScenario::DryRunJson => args.extend(["-d", "-f", "json"]),
        AliasScenario::RouteProblem => {
            env.write_config(&format!(
                "gmail:\n  account: {ACCOUNT}\n  writer: {HOST}\n  sync:\n    route_prefixes: [\"#\"]\n"
            ));
            fake.set_labels(serde_json::json!([
                {"id": "Label_1", "name": "#/missing", "type": "user"},
                {"id": "INBOX", "name": "INBOX", "type": "system"}
            ]));
            args.extend(["-f", "json"]);
        }
        AliasScenario::Help => args.push("--help"),
        AliasScenario::SetupFailure => args.extend(["-l", "501", "-f", "json"]),
    }
    let output = run_gmail(&env, &fake, &args);
    let root = env.root();
    let vault = snapshot_tree(env.vault(), true)
        .into_iter()
        .map(|(path, bytes)| (path, normalize_root(&bytes, root)))
        .collect();
    let state_root = root.join("state");
    let state = snapshot_tree(&state_root, false)
        .into_iter()
        .map(|(path, bytes)| (path, normalize_root(&bytes, root)))
        .collect();
    AliasSnapshot {
        status: output.status.code(),
        stdout: normalize_root(&output.stdout, root),
        stderr: normalize_root(&output.stderr, root),
        vault,
        state,
        git_subjects: git_subjects(env.vault()),
    }
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
    assert_eq!(threads_list_queries(&fake), ["label:bob"]);

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

    let before_vault = snapshot_tree(env.vault(), true);
    let before_state = snapshot_tree(&env.root().join("state"), false);
    let output = run_gmail(&env, &fake, &["sync", "-d", "-f", "json"]);
    assert_success(&output, "dry-run sync");
    let value = json_stdout(&output);
    assert_eq!(value["dry_run"], true);
    assert_eq!(value["results"][0]["status"], "would_file");
    assert_eq!(value["summary"]["watching"], 1);
    assert!(!env.vault().join("gmail_inbox.md").exists());
    assert!(!env.vault().join("mail").exists());
    assert!(!env.vault().join(".bob/gmail").exists());
    assert_eq!(snapshot_tree(env.vault(), true), before_vault);
    assert_eq!(
        snapshot_tree(&env.root().join("state"), false),
        before_state
    );

    let live = run_gmail(&env, &fake, &["sync", "-f", "json"]);
    assert_success(&live, "live sync after dry run");
    let mut dry_result = value["results"][0].clone();
    dry_result["status"] = serde_json::json!("filed");
    assert_eq!(dry_result, json_stdout(&live)["results"][0]);
}

#[test]
fn hidden_pull_alias_matches_sync_byte_for_byte_across_scenarios() {
    for scenario in [
        AliasScenario::LiveSuccess,
        AliasScenario::DryRunHuman,
        AliasScenario::DryRunJson,
        AliasScenario::RouteProblem,
        AliasScenario::Help,
        AliasScenario::SetupFailure,
    ] {
        let sync = run_alias_fixture("sync", scenario);
        let pull = run_alias_fixture("pull", scenario);
        assert_eq!(sync, pull, "sync/pull parity for {scenario:?}");
    }
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
    assert!(!fake.called_with(&["gmail.users.threads.list"]));

    let dry_run = run_gmail(&env, &fake, &["sync", "-d", "-f", "json"]);
    assert_success(&dry_run, "non-writer dry-run");
    assert_eq!(json_stdout(&dry_run)["dry_run"], true);
    assert_eq!(json_stdout(&dry_run)["pulled"], 1);
    assert!(!env.vault().join("gmail_inbox.md").exists());

    fake.clear_calls();
    env.write_config(&format!(
        "gmail:\n  account: {ACCOUNT}\n  writer: {HOST}\n  pull:\n    label: ''\n"
    ));
    let disabled = run_gmail(&env, &fake, &["sync", "-f", "json"]);
    assert_eq!(disabled.status.code(), Some(2));
    let value: Value = serde_json::from_slice(&disabled.stdout).unwrap();
    assert_eq!(value["error"]["kind"], "sync_disabled");
    assert!(!fake.called_with(&["gmail.users.threads.list"]));
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

    let before_vault = snapshot_tree(env.vault(), true);
    let before_state = snapshot_tree(&env.root().join("state"), false);

    // Fast contention budget: a held guard must skip without waiting the
    // default 120 s twice.
    let json = run_gmail(&env, &fake, &["sync", "-f", "json", "-r", "0"]);
    assert_success(&json, "busy json skip");
    let value = json_stdout(&json);
    assert_eq!(value["ok"], true);
    assert_eq!(value["pulled"], 0);
    assert!(value["results"].as_array().unwrap().is_empty());

    let human = run_gmail(&env, &fake, &["sync", "-q", "-r", "0"]);
    assert_success(&human, "busy quiet skip");
    assert!(stdout(&human).is_empty(), "{}", stdout(&human));
    assert!(!env.vault().join("gmail_inbox.md").exists());
    assert_eq!(snapshot_tree(env.vault(), true), before_vault);
    assert_eq!(
        snapshot_tree(&env.root().join("state"), false),
        before_state
    );
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
    assert!(
        sync_text.contains("watch-only"),
        "limit help should name watch-only -l 0:\n{sync_text}"
    );
    assert!(
        help.contains("bob gmail sync -l 0"),
        "parent help should show the watch-only example:\n{help}"
    );
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

#[test]
fn sync_pages_threads_list_by_page_token() {
    let env = GmailEnv::new("bob-cli-gmail-sync-pages");
    let fake = FakeGog::new(&env, "gog").scenario("gmail_pull_many");
    fake.set_search_pages(serde_json::json!([
        {
            "threads": [{"id": "0123456789abcdef"}],
            "nextPageToken": "page-2",
            "resultSizeEstimate": 2
        },
        {
            "threads": [{"id": "aaaaaaaaaaaaaaaa"}],
            "resultSizeEstimate": 2
        }
    ]));

    let output = run_gmail(&env, &fake, &["sync", "-l", "2", "-f", "json"]);
    assert_success(&output, "paged sync");
    let value = json_stdout(&output);
    assert_eq!(value["ok"], true);
    assert_eq!(value["pulled"], 2);
    assert_eq!(threads_list_queries(&fake), ["label:bob", "label:bob"]);
    assert_eq!(
        threads_list_page_tokens(&fake),
        [None, Some("page-2".to_string())]
    );
}

#[test]
fn sync_empty_label_is_ok_with_no_threads_key() {
    let env = GmailEnv::new("bob-cli-gmail-sync-empty");
    let fake = FakeGog::new(&env, "gog");
    fake.set_search_pages(serde_json::json!([{"resultSizeEstimate": 0}]));

    let output = run_gmail(&env, &fake, &["sync", "-f", "json"]);
    assert_success(&output, "empty label sync");
    let value = json_stdout(&output);
    assert_eq!(value["ok"], true);
    assert_eq!(value["pulled"], 0);
    assert!(value["warnings"].as_array().unwrap().is_empty(), "{value}");
    assert!(fake.called_with(&["gmail.users.threads.list"]));
}

#[test]
fn sync_threads_list_refused_is_a_search_failure() {
    let env = GmailEnv::new("bob-cli-gmail-sync-refused");
    let fake = FakeGog::new(&env, "gog").scenario("threads_list_refused");

    let output = run_gmail(&env, &fake, &["sync", "-f", "json"]);
    assert_eq!(output.status.code(), Some(1), "{}", stdout(&output));
    let value = json_stdout(&output);
    assert_eq!(value["ok"], false);
    let warnings = value["warnings"].as_array().expect("warnings");
    assert!(
        warnings.iter().any(|warning| {
            warning.as_str().is_some_and(|text| {
                text.contains("gog_usage")
                    && text.contains(
                        "gog's command policy refused Bob's Gmail command",
                    )
            })
        }),
        "{warnings:?}"
    );
}

#[test]
fn sync_adopts_before_limit_and_skips_candidate_fetch_for_the_adoption() {
    let env = GmailEnv::new("bob-cli-gmail-sync-adopt-limit");
    let fake = FakeGog::new(&env, "gog").scenario("gmail_pull_many");
    seed_unenrolled_task(&env, &fake, "0123456789abcdef", "gmail_inbox.md");
    fake.set_search_pages(serde_json::json!([
        {"threads": [{"id": "0123456789abcdef"}, {"id": "aaaaaaaaaaaaaaaa"}]}
    ]));
    fake.clear_calls();

    let output =
        run_gmail(&env, &fake, &["sync", "-l", "1", "-r", "0", "-f", "json"]);
    assert_success(&output, "adopt and file one new thread");
    let value = json_stdout(&output);
    assert_eq!(value["pulled"], 1);
    assert_eq!(value["summary"]["watching"], 2);
    assert!(value["results"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["status"] == "adopted"));
    assert!(value["results"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["status"] == "filed"));
    assert_eq!(task_row_count(&env.vault().join("gmail_inbox.md")), 2);
    assert_eq!(
        journal_kinds(&env)
            .iter()
            .filter(|kind| *kind == "gmail.adopted")
            .count(),
        1
    );
    assert!(
        !fake.calls().iter().any(|call| {
            call.windows(3)
                .any(|args| args == ["gmail", "thread", "get"])
                && call.iter().any(|arg| arg == "0123456789abcdef")
        }),
        "adopted thread was fetched as a candidate: {:?}",
        fake.calls()
    );

    let replay =
        run_gmail(&env, &fake, &["sync", "-l", "1", "-r", "0", "-f", "json"]);
    assert_success(&replay, "replay after adoption");
    assert_eq!(json_stdout(&replay)["pulled"], 0);
    assert_eq!(task_row_count(&env.vault().join("gmail_inbox.md")), 2);
    assert_eq!(
        journal_kinds(&env)
            .iter()
            .filter(|kind| *kind == "gmail.adopted")
            .count(),
        1
    );
}

#[test]
fn sync_adopts_without_search_presence_and_in_watch_only_mode() {
    for (name, limit, search_ids, expected_pulled, expected_watching) in [
        ("absent", "1", vec!["aaaaaaaaaaaaaaaa"], 1, 2),
        ("watch-only", "0", vec!["aaaaaaaaaaaaaaaa"], 0, 1),
    ] {
        let env = GmailEnv::new(&format!("bob-cli-gmail-sync-adopt-{name}"));
        let fake = FakeGog::new(&env, "gog").scenario("gmail_pull_many");
        seed_unenrolled_task(&env, &fake, "0123456789abcdef", "gmail_inbox.md");
        fake.set_search_pages(serde_json::json!([{
            "threads": search_ids.iter().map(|id| serde_json::json!({"id": id})).collect::<Vec<_>>()
        }]));
        fake.clear_calls();

        let output = run_gmail(
            &env,
            &fake,
            &["sync", "-l", limit, "-r", "0", "-f", "json"],
        );
        assert_success(&output, name);
        let value = json_stdout(&output);
        assert_eq!(value["pulled"], expected_pulled);
        assert_eq!(value["summary"]["watching"], expected_watching);
        assert_eq!(
            journal_kinds(&env)
                .iter()
                .filter(|kind| *kind == "gmail.adopted")
                .count(),
            1
        );
        if limit == "0" {
            assert!(!fake.called_with(&["gmail.users.threads.list"]));
        }
        assert!(!fake.calls().iter().any(|call| {
            call.windows(3)
                .any(|args| args == ["gmail", "thread", "get"])
                && call.iter().any(|arg| arg == "0123456789abcdef")
        }));
    }
}

#[test]
fn sync_adopts_a_task_outside_the_inbox() {
    let env = GmailEnv::new("bob-cli-gmail-sync-adopt-area");
    let fake = FakeGog::new(&env, "gog").scenario("gmail_pull_one");
    seed_unenrolled_task(&env, &fake, "0123456789abcdef", "job.md");

    let output = run_gmail(&env, &fake, &["sync", "-r", "0", "-f", "json"]);
    assert_success(&output, "adopt area task");
    assert_eq!(json_stdout(&output)["pulled"], 0);
    assert_eq!(task_row_count(&env.vault().join("job.md")), 1);
    assert!(!env.vault().join("gmail_inbox.md").exists());
    assert_eq!(
        journal_kinds(&env)
            .iter()
            .filter(|kind| *kind == "gmail.adopted")
            .count(),
        1
    );
}

#[test]
fn legacy_pull_enrollment_is_reported_without_filing_again() {
    let env = GmailEnv::new("bob-cli-gmail-sync-legacy");
    let fake = FakeGog::new(&env, "gog").scenario("gmail_pull_one");
    let journal = env.vault().join(".bob/gmail/legacy-host.jsonl");
    fs::create_dir_all(journal.parent().unwrap()).expect("journal directory");
    fs::write(
        &journal,
        serde_json::json!({
            "v": 1,
            "ts": "2026-11-01T09:00:00-04:00",
            "host": "legacy-host",
            "kind": "gmail.pulled",
            "value": {
                "account": ACCOUNT,
                "thread_id": "0123456789abcdef",
                "inbox": "gmail_inbox.md",
                "note": "mail/2026/0123456789abcdef.md"
            }
        })
        .to_string()
            + "\n",
    )
    .expect("legacy enrollment");

    let output = run_gmail(&env, &fake, &["sync", "-r", "0", "-f", "json"]);
    assert_success(&output, "legacy enrollment sync");
    let value = json_stdout(&output);
    assert_eq!(value["results"][0]["status"], "already_pulled");
    assert_eq!(value["pulled"], 0);
    assert!(!env.vault().join("gmail_inbox.md").exists());
    assert_eq!(
        fake.calls()
            .iter()
            .filter(|call| call.iter().any(|arg| arg == "thread"))
            .count(),
        0
    );
}

#[test]
fn enrolled_tasks_close_delete_reopen_move_and_survive_label_removal() {
    let env = GmailEnv::new("bob-cli-gmail-sync-lifecycle");
    let fake = FakeGog::new(&env, "gog").scenario("gmail_pull_one");
    let first = run_gmail(&env, &fake, &["sync", "-r", "0", "-f", "json"]);
    assert_success(&first, "initial filing");
    let first_value = json_stdout(&first);
    assert_eq!(first_value["pulled"], 1);
    let link = first_value["results"][0]["link"]
        .as_str()
        .expect("filed mail link")
        .to_string();
    let inbox_path = env.vault().join("gmail_inbox.md");

    let inbox = fs::read_to_string(&inbox_path).expect("filed inbox");
    fs::write(&inbox_path, inbox.replacen("- [ ] #task", "- [x] #task", 1))
        .expect("close task");
    let closed = run_gmail(&env, &fake, &["sync", "-r", "0", "-f", "json"]);
    assert_success(&closed, "closed enrolled task");
    assert_eq!(json_stdout(&closed)["pulled"], 0);
    assert_eq!(json_stdout(&closed)["summary"]["watching"], 0);
    assert_eq!(task_row_count(&inbox_path), 1);

    let inbox = fs::read_to_string(&inbox_path).expect("closed task note");
    fs::write(&inbox_path, inbox.replacen("- [x] #task", "- [ ] #task", 1))
        .expect("reopen task");
    let reopened = run_gmail(&env, &fake, &["sync", "-r", "0", "-f", "json"]);
    assert_success(&reopened, "reopened enrolled task");
    assert_eq!(json_stdout(&reopened)["summary"]["watching"], 1);
    assert_eq!(task_row_count(&inbox_path), 1);

    let inbox = fs::read_to_string(&inbox_path).expect("reopened task note");
    let row = inbox
        .lines()
        .find(|line| line.contains("#task [[mail/"))
        .unwrap()
        .to_string();
    let remaining = inbox
        .lines()
        .filter(|line| *line != row)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(&inbox_path, remaining).expect("delete task while label remains");
    let deleted = run_gmail(&env, &fake, &["sync", "-r", "0", "-f", "json"]);
    assert_success(&deleted, "deleted enrolled task with label");
    assert_eq!(json_stdout(&deleted)["pulled"], 0);
    assert_eq!(json_stdout(&deleted)["summary"]["watching"], 0);
    assert_eq!(task_row_count(&inbox_path), 0);

    let mut inbox = fs::read_to_string(&inbox_path).expect("empty inbox note");
    inbox.push_str(&format!("- [ ] #task Reopened {link}\n"));
    fs::write(&inbox_path, inbox).expect("link an open task without the label");
    fake.set_search_pages(serde_json::json!([{"resultSizeEstimate": 0}]));
    let relinked =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_success(&relinked, "relinked enrolled task without label");
    assert_eq!(json_stdout(&relinked)["summary"]["watching"], 1);

    let inbox = fs::read_to_string(&inbox_path).expect("relinked task note");
    let row = inbox
        .lines()
        .find(|line| line.contains("#task") && line.contains("[[mail/"))
        .unwrap()
        .to_string();
    let remaining = inbox
        .lines()
        .filter(|line| *line != row)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(&inbox_path, remaining).expect("remove moved task from inbox");
    write_area(&env, "job");
    let job_path = env.vault().join("job.md");
    let mut job = fs::read_to_string(&job_path).expect("job area");
    job.push_str(&format!("{row}\n"));
    fs::write(&job_path, job).expect("move task into area");
    fake.set_search_pages(serde_json::json!([{"resultSizeEstimate": 0}]));
    let moved = run_gmail(&env, &fake, &["sync", "-r", "0", "-f", "json"]);
    assert_success(&moved, "moved enrolled task with removed label");
    assert_eq!(json_stdout(&moved)["summary"]["watching"], 1);
    assert_eq!(task_row_count(&job_path), 1);
    assert_eq!(task_row_count(&inbox_path), 0);

    fs::write(
        &job_path,
        fs::read_to_string(&job_path).unwrap().replace(&row, ""),
    )
    .expect("delete enrolled task");
    let deleted = run_gmail(&env, &fake, &["sync", "-r", "0", "-f", "json"]);
    assert_success(&deleted, "deleted moved enrolled task");
    assert_eq!(json_stdout(&deleted)["pulled"], 0);
    assert_eq!(json_stdout(&deleted)["summary"]["watching"], 0);
    assert_eq!(task_row_count(&job_path), 0);
    assert_eq!(
        journal_kinds(&env)
            .iter()
            .filter(|kind| *kind == "gmail.filed")
            .count(),
        1
    );
}

#[test]
fn sync_limits_cover_default_zero_maximum_and_invalid_value() {
    for (name, args, expected, expected_exit) in [
        ("default", vec!["sync", "-r", "0", "-f", "json"], 2, 0),
        (
            "maximum",
            vec!["sync", "-l", "500", "-r", "0", "-f", "json"],
            2,
            0,
        ),
        (
            "invalid",
            vec!["sync", "-l", "501", "-r", "0", "-f", "json"],
            0,
            2,
        ),
    ] {
        let env = GmailEnv::new(&format!("bob-cli-gmail-sync-limit-{name}"));
        let fake = FakeGog::new(&env, "gog").scenario("gmail_pull_many");
        let before_vault = snapshot_tree(env.vault(), true);
        let before_state = snapshot_tree(&env.root().join("state"), false);
        let output = run_gmail(&env, &fake, &args);
        assert_eq!(
            output.status.code(),
            Some(expected_exit),
            "{name}: {}",
            stdout(&output)
        );
        if expected_exit == 0 {
            assert_eq!(json_stdout(&output)["pulled"], expected);
        } else {
            assert_eq!(snapshot_tree(env.vault(), true), before_vault);
            assert_eq!(
                snapshot_tree(&env.root().join("state"), false),
                before_state
            );
            assert!(
                fake.calls().is_empty(),
                "limit validation must precede Gmail reads"
            );
        }
    }
}

#[test]
fn sync_keeps_partial_success_retries_transients_and_aborts_hydration_auth() {
    let env = GmailEnv::new("bob-cli-gmail-sync-partial");
    let fake = FakeGog::new(&env, "gog").scenario("gmail_pull_many");
    fake.set_thread_fail(
        "0123456789abcdef",
        "notFound: synthetic thread missing",
        1,
    );
    let partial = run_gmail(&env, &fake, &["sync", "-r", "0", "-f", "json"]);
    assert_eq!(partial.status.code(), Some(1), "{}", stdout(&partial));
    let partial_value = json_stdout(&partial);
    assert_eq!(partial_value["ok"], false);
    assert_eq!(partial_value["pulled"], 1);
    assert_eq!(task_row_count(&env.vault().join("gmail_inbox.md")), 1);

    let retry_env = GmailEnv::new("bob-cli-gmail-sync-transient");
    let retry_fake = FakeGog::new(&retry_env, "gog").scenario("gmail_pull_one");
    retry_fake.set_transient_failure(
        "thread_get",
        "backendError: retry this read",
        1,
        1,
    );
    let retried = run_gmail(&retry_env, &retry_fake, &["sync", "-f", "json"]);
    assert_success(&retried, "transient thread read recovers");
    assert_eq!(json_stdout(&retried)["pulled"], 1);
    assert!(
        retry_fake
            .calls()
            .iter()
            .filter(|call| {
                call.windows(3)
                    .any(|args| args == ["gmail", "thread", "get"])
            })
            .count()
            >= 3,
        "metadata retry and hydration should all run"
    );

    let auth_env = GmailEnv::new("bob-cli-gmail-sync-hydration-auth");
    let auth_fake = FakeGog::new(&auth_env, "gog").scenario("gmail_pull_one");
    auth_fake.set_thread_full_fail(
        "0123456789abcdef",
        "invalid_grant: synthetic revoked grant during hydration",
        1,
    );
    let before = snapshot_tree(auth_env.vault(), true);
    let auth =
        run_gmail(&auth_env, &auth_fake, &["sync", "-r", "0", "-f", "json"]);
    assert_eq!(auth.status.code(), Some(1), "{}", stdout(&auth));
    assert_eq!(snapshot_tree(auth_env.vault(), true), before);
    assert!(!journal_kinds(&auth_env)
        .iter()
        .any(|kind| kind == "gmail.filed"));
    assert!(
        auth_fake.calls().iter().any(|call| {
            call.windows(3)
                .any(|args| args == ["gmail", "thread", "get"])
                && call.contains(&"--full".to_string())
        }),
        "test must fail during hydration, after metadata"
    );
}

#[test]
fn busy_vault_lock_skips_without_vault_or_journal_writes() {
    use fs2::FileExt;
    let env = GmailEnv::new("bob-cli-gmail-sync-busy-vault");
    let fake = FakeGog::new(&env, "gog").scenario("gmail_pull_one");
    let lock_path = env.root().join("bob_sync.lock");
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)
        .expect("open vault sync lock");
    lock.try_lock_exclusive().expect("hold vault lock");
    let before = snapshot_tree(env.vault(), true);
    let journal_before = journal_kinds(&env);

    let output = run_gmail(&env, &fake, &["sync", "-q", "-r", "0"]);
    assert_success(&output, "busy vault-lock skip");
    assert!(stdout(&output).is_empty());
    assert_eq!(snapshot_tree(env.vault(), true), before);
    assert_eq!(journal_kinds(&env), journal_before);
    assert!(fake.called_with(&["gmail.users.threads.list"]));
    drop(lock);
}

#[test]
fn sync_commits_only_owned_paths_and_noop_replay_keeps_head_and_journal() {
    let env = GmailEnv::new("bob-cli-gmail-sync-git");
    initialize_git_vault(env.vault());
    let fake = FakeGog::new(&env, "gog").scenario("gmail_pull_one");
    fs::write(env.vault().join("unrelated.md"), "uncommitted user edit\n")
        .expect("unrelated dirt");

    let first = run_gmail(&env, &fake, &["sync", "-r", "0", "-f", "json"]);
    assert_success(&first, "Git-backed sync");
    assert_eq!(git_subjects(env.vault())[0], "gmail: sync 1 threads");
    let committed =
        git(env.vault(), &["show", "--format=", "--name-only", "HEAD"]);
    let changed = String::from_utf8(committed.stdout).unwrap();
    assert!(changed.contains("gmail_inbox.md"), "{changed}");
    assert!(
        !changed.contains("unrelated.md"),
        "unrelated dirt entered the sync commit: {changed}"
    );
    let status =
        String::from_utf8(git(env.vault(), &["status", "--short"]).stdout)
            .unwrap();
    assert!(status.contains("unrelated.md"), "{status}");
    let head =
        String::from_utf8(git(env.vault(), &["rev-parse", "HEAD"]).stdout)
            .unwrap();
    let journal = journal_kinds(&env);

    let second = run_gmail(&env, &fake, &["sync", "-r", "0", "-f", "json"]);
    assert_success(&second, "no-op replay");
    assert_eq!(
        String::from_utf8(git(env.vault(), &["rev-parse", "HEAD"]).stdout)
            .unwrap(),
        head
    );
    assert_eq!(journal_kinds(&env), journal);
}

fn install_failing_pre_commit(vault: &Path) {
    let hook = vault.join(".git/hooks/pre-commit");
    fs::create_dir_all(hook.parent().unwrap()).expect("hooks dir");
    fs::write(&hook, "#!/bin/sh\nexit 1\n").expect("write hook");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut permissions = fs::metadata(&hook).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&hook, permissions).unwrap();
    }
}

#[test]
fn metadata_auth_abort_writes_nothing_after_a_prepared_candidate() {
    let env = GmailEnv::new("bob-cli-gmail-sync-metadata-auth");
    let fake = FakeGog::new(&env, "gog").scenario("gmail_pull_many");
    fake.set_thread_fail(
        "aaaaaaaaaaaaaaaa",
        "invalid_grant: synthetic revoked grant during metadata",
        1,
    );
    let before = snapshot_tree(env.vault(), true);
    let output = run_gmail(&env, &fake, &["sync", "-r", "0", "-f", "json"]);
    assert_eq!(output.status.code(), Some(1), "{}", stdout(&output));
    assert_eq!(snapshot_tree(env.vault(), true), before);
    assert!(!journal_kinds(&env).iter().any(|kind| kind == "gmail.filed"));
}

#[test]
fn policy_label_lookup_auth_abort_writes_nothing() {
    let env = GmailEnv::new("bob-cli-gmail-sync-policy-auth");
    env.write_config(&format!(
        "gmail:\n  account: {ACCOUNT}\n  writer: {HOST}\n  never_save:\n    labels: [secret]\n"
    ));
    let fake = FakeGog::new(&env, "gog").scenario("gmail_pull_one");
    fake.set_fail_after(
        "labels_list",
        "invalid_grant: synthetic revoked grant during policy lookup",
        1,
        1,
    );
    let before = snapshot_tree(env.vault(), true);
    let output = run_gmail(&env, &fake, &["sync", "-r", "0", "-f", "json"]);
    assert_eq!(output.status.code(), Some(1), "{}", stdout(&output));
    assert_eq!(snapshot_tree(env.vault(), true), before);
    assert!(!journal_kinds(&env).iter().any(|kind| kind == "gmail.filed"));
    let labels_calls = fake
        .calls()
        .iter()
        .filter(|call| call.iter().any(|arg| arg.contains("labels")))
        .count();
    assert!(
        labels_calls >= 2,
        "policy lookup must run a second labels.list: {:?}",
        fake.calls()
    );
}

#[test]
fn search_failure_still_adopts_and_replays_recovery() {
    let env = GmailEnv::new("bob-cli-gmail-sync-search-fail-adopt");
    let fake = FakeGog::new(&env, "gog").scenario("gmail_pull_one");
    seed_unenrolled_task(&env, &fake, "0123456789abcdef", "job.md");
    fake.set_failure("api_call", "notFound: synthetic label search missing", 1);
    let output = run_gmail(&env, &fake, &["sync", "-r", "0", "-f", "json"]);
    assert_eq!(output.status.code(), Some(1), "{}", stdout(&output));
    let value = json_stdout(&output);
    assert_eq!(value["ok"], false);
    assert!(
        journal_kinds(&env)
            .iter()
            .any(|kind| kind == "gmail.adopted"),
        "local adoption still runs after search failure: {:?}",
        journal_kinds(&env)
    );
    assert_eq!(task_row_count(&env.vault().join("job.md")), 1);
}

#[test]
fn idle_limit_zero_unreadable_recovery_fails() {
    let env = GmailEnv::new("bob-cli-gmail-sync-unreadable-recovery");
    initialize_git_vault(env.vault());
    let fake = FakeGog::new(&env, "gog").scenario("gmail_pull_one");
    install_failing_pre_commit(env.vault());
    let failed = run_gmail(&env, &fake, &["sync", "-r", "0", "-f", "json"]);
    assert_eq!(failed.status.code(), Some(1), "{}", stdout(&failed));
    let state = env.root().join("state/bob/gmail");
    let evidence = fs::read_dir(&state)
        .expect("state dir")
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name().and_then(|name| name.to_str()).is_some_and(
                |name| {
                    name.starts_with("sync-recovery-")
                        && name.ends_with(".json")
                },
            )
        })
        .expect("recovery evidence after commit failure");
    fs::remove_file(&evidence).expect("remove evidence file");
    fs::create_dir(&evidence).expect("directory at evidence path");
    fs::remove_file(env.vault().join(".git/hooks/pre-commit")).ok();
    let output =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_eq!(output.status.code(), Some(1), "{}", stdout(&output));
    let value = json_stdout(&output);
    assert_eq!(value["ok"], false);
    let stderr = stderr(&output);
    assert!(
        value["warnings"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|warning| warning
                .as_str()
                .is_some_and(|text| { text.contains("recovery") }))
            || stderr.contains("recovery"),
        "unreadable recovery must be a body-free failure: {value} {stderr}"
    );
}

#[test]
fn healthy_two_thread_zero_retry_budget_files_both() {
    let env = GmailEnv::new("bob-cli-gmail-sync-two-thread-r0");
    let fake = FakeGog::new(&env, "gog").scenario("gmail_pull_many");
    let output = run_gmail(&env, &fake, &["sync", "-r", "0", "-f", "json"]);
    assert_success(&output, "two-thread -r 0");
    let value = json_stdout(&output);
    assert_eq!(value["ok"], true);
    assert_eq!(value["pulled"], 2);
    assert_eq!(task_row_count(&env.vault().join("gmail_inbox.md")), 2);
}

#[test]
fn hydration_give_up_keeps_prepared_success_and_fails() {
    let env = GmailEnv::new("bob-cli-gmail-sync-hydration-giveup");
    let fake = FakeGog::new(&env, "gog").scenario("gmail_pull_many");
    fake.set_thread_full_fail(
        "aaaaaaaaaaaaaaaa",
        "backendError: synthetic hydration timeout",
        1,
    );
    let output = run_gmail(&env, &fake, &["sync", "-r", "0", "-f", "json"]);
    assert_eq!(output.status.code(), Some(1), "{}", stdout(&output));
    let value = json_stdout(&output);
    assert_eq!(value["ok"], false);
    assert_eq!(value["pulled"], 1);
    assert_eq!(task_row_count(&env.vault().join("gmail_inbox.md")), 1);
}
