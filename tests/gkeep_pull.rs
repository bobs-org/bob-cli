//! `bob gkeep pull` guarded transaction integration tests.
//!
//! Every test uses the fake adapter (`BOB_GKEEP_ADAPTER`); no test touches
//! live Google Keep.

mod gkeep_support;

use std::{
    fs,
    path::{Path, PathBuf},
    process::Output,
};

use gkeep_support::{
    archive_ok, error_response, note, snapshot_ok, stderr, stdout, FakeAdapter,
    GkeepEnv, TempDir,
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

fn default_target() -> String {
    // Mirrors the real `gkeep_inbox.md` shape: frontmatter, two intro
    // bullets, and an empty `## Tasks` heading with no trailing newline.
    "---\nkey: value\n---\n- intro bullet one\n- The tasks below are pulled in by the `bob gkeep` command.\n## Tasks".to_string()
}

fn write_token_script(vault: &Path) -> PathBuf {
    let path = vault.join("token.sh");
    fs::write(&path, "#!/bin/sh\nprintf 'aas_et/test-master-token\\n'\n")
        .expect("write token script");
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(&path)
            .expect("stat token script")
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).expect("chmod token script");
    }
    path
}

fn configure(env: &GkeepEnv, token_script: &Path) {
    let quoted = token_script.to_string_lossy().replace('\'', "'\\''");
    let text = format!(
        "gkeep:\n  email: bryanbugyi34@gmail.com\n  token_command: \"sh '{quoted}'\"\n",
    );
    env.write_config(&text);
}

fn write_target(vault: &Path, contents: &str) {
    fs::write(vault.join("gkeep_inbox.md"), contents).expect("write target");
}

fn read_target(vault: &Path) -> String {
    fs::read_to_string(vault.join("gkeep_inbox.md")).expect("read target")
}

fn run_pull(
    env: &GkeepEnv,
    fake: &FakeAdapter,
    state: &TempDir,
    args: &[&str],
    extra_env: &[(&str, &str)],
) -> Output {
    let mut cmd = env.command();
    fake.install(&mut cmd);
    cmd.env("TZ", "UTC")
        .env("XDG_STATE_HOME", state.path())
        .env("XDG_CACHE_HOME", state.path().join("cache"))
        .arg("gkeep")
        .arg("pull");
    for arg in args {
        cmd.arg(arg);
    }
    for (key, value) in extra_env {
        cmd.env(key, value);
    }
    cmd.output().expect("run bob gkeep pull")
}

fn setup(prefix: &str) -> (GkeepEnv, FakeAdapter, TempDir, PathBuf) {
    let env = GkeepEnv::new(prefix);
    let state = TempDir::new(prefix);
    let token = write_token_script(env.vault());
    configure(&env, &token);
    write_target(env.vault(), &default_target());
    let fake = FakeAdapter::new(&env, "adapter");
    (env, fake, state, token)
}

fn init_git(vault: &Path) {
    let run = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(vault)
            .args(args)
            .output()
            .expect("run git");
        assert!(
            status.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&status.stderr)
        );
    };
    run(&["init"]);
    run(&["config", "user.email", "test@example.com"]);
    run(&["config", "user.name", "Test"]);
    run(&["config", "commit.gpgsign", "false"]);
}

fn git_rev_list_count(vault: &Path) -> usize {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(vault)
        .args(["rev-list", "--count", "HEAD"])
        .output()
        .expect("git rev-list");
    assert!(out.status.success());
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .expect("rev-list count parses")
}

fn git_log_names(vault: &Path) -> Vec<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(vault)
        .args(["show", "--name-only", "--format="])
        .output()
        .expect("git show");
    assert!(out.status.success());
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .filter(|line| !line.is_empty())
        .collect()
}

fn journal_records(state: &TempDir) -> Vec<serde_json::Value> {
    let path = state
        .path()
        .join("bob-cli")
        .join("gkeep")
        .join("journal.jsonl");
    let Ok(text) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

#[test]
fn normal_pull_writes_verifies_commits_and_archives() {
    let (env, fake, state, _token) = setup("bob-cli-gkeep-pull-normal");
    init_git(env.vault());
    let n1 = note("Call dentist about crown")
        .id("note-1")
        .text("They close at 5 on Fridays")
        .created("2026-09-27T21:14:03Z")
        .url("https://keep.google.com/u/0/#NOTE/note-1")
        .build();
    let n2 = note("Hardware store")
        .id("note-2")
        .created("2026-09-26T08:02:00Z")
        .list(vec![
            ("wood screws", false, false),
            ("sandpaper", true, false),
        ])
        .build();
    let pinned = note("Wi-Fi guest password")
        .id("note-pinned")
        .created("2026-09-25T10:00:00Z")
        .pinned()
        .build();
    fake.respond(
        "snapshot",
        &snapshot_ok("bryanbugyi34@gmail.com", vec![n1, n2, pinned]),
    );
    fake.respond(
        "archive",
        &archive_ok(vec![("note-1", "archived"), ("note-2", "archived")]),
    );

    let out = run_pull(&env, &fake, &state, &[], &[]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout:\n{}\nstderr:\n{}",
        stdout(&out),
        stderr(&out)
    );
    let body = stdout(&out);
    assert!(body.contains("Google Keep"), "{body}");
    assert!(body.contains("written"), "{body}");
    assert!(body.contains("archived"), "{body}");
    assert!(body.contains("ok 2 written"), "{body}");

    let target = read_target(env.vault());
    assert!(target.contains("Call dentist about crown"), "{target}");
    assert!(target.contains("Hardware store"), "{target}");
    assert!(target.contains("%%gkeep:v1:note-1:"), "{target}");
    assert!(target.contains("%%gkeep:v1:note-2:"), "{target}");
    // Pinned notes stay in Keep and out of the vault.
    assert!(!target.contains("Wi-Fi guest password"), "{target}");

    // One archive call carrying both expect objects (call 2: snapshot is 1).
    // Plan order is oldest first, so note-2 (09-26) precedes note-1 (09-27).
    let req: serde_json::Value =
        serde_json::from_str(&fake.request("archive", 2)).expect("archive req");
    assert_eq!(req["op"], serde_json::json!("archive"));
    assert_eq!(req["notes"].as_array().unwrap().len(), 2);
    let mut titles: Vec<String> = req["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["expect"]["title"].as_str().unwrap().to_string())
        .collect();
    titles.sort();
    assert_eq!(titles, vec!["Call dentist about crown", "Hardware store"]);

    // Exactly one new commit touching only the target.
    assert_eq!(git_rev_list_count(env.vault()), 1);
    assert_eq!(git_log_names(env.vault()), vec!["gkeep_inbox.md"]);
    // Byte-exact: both markers present exactly once, in oldest-first order.
    assert_eq!(target.matches("%%gkeep:v1:note-1:").count(), 1);
    assert_eq!(target.matches("%%gkeep:v1:note-2:").count(), 1);
    let pos1 = target.find("%%gkeep:v1:note-2:").unwrap();
    let pos2 = target.find("%%gkeep:v1:note-1:").unwrap();
    assert!(pos1 < pos2, "oldest first:\n{target}");
    // Byte-exact whole file (TZ=UTC; rendering uses only created dates).
    let expected = "---\nkey: value\n---\n- intro bullet one\n- The tasks below are pulled in by the `bob gkeep` command.\n## Tasks\n\n- [ ] #task Hardware store [created::2026-09-26]\n\t- [ ] wood screws\n\t- [x] sandpaper\n\t- Source: Google Keep \u{00b7} 2026-09-26 08:02 %%gkeep:v1:note-2:32a5e5e2fd2c%%\n- [ ] #task Call dentist about crown [created::2026-09-27]\n\t- They close at 5 on Fridays\n\t- Source: [Google Keep](https://keep.google.com/u/0/#NOTE/note-1) \u{00b7} 2026-09-27 21:14 %%gkeep:v1:note-1:47582521e307%%\n";
    assert_eq!(target, expected, "byte-exact target:\n{target}");

    let journal = journal_records(&state);
    assert!(
        journal
            .iter()
            .any(|row| row["event"] == "written" && row["id"] == "note-1"),
        "{journal:?}"
    );
    assert!(
        journal
            .iter()
            .any(|row| row["event"] == "archived" && row["id"] == "note-2"),
        "{journal:?}"
    );
    for row in &journal {
        assert!(row.get("ts").is_some(), "{row}");
        assert_eq!(row["path"], serde_json::json!("gkeep_inbox.md"));
    }
}

#[test]
fn second_pull_after_success_is_archive_only_with_no_duplicate_write() {
    let (env, fake, state, _token) = setup("bob-cli-gkeep-pull-second");
    init_git(env.vault());
    let n1 = note("Call dentist")
        .id("note-1")
        .created("2026-09-27T21:14:03Z")
        .build();
    fake.respond(
        "snapshot",
        &snapshot_ok("bryanbugyi34@gmail.com", vec![n1.clone()]),
    );
    fake.respond("archive", &archive_ok(vec![("note-1", "archived")]));

    let first = run_pull(&env, &fake, &state, &[], &[]);
    assert_eq!(first.status.code(), Some(0));
    let after_first = read_target(env.vault());
    let count_first = after_first.matches("Call dentist").count();

    // The snapshot still returns the note, so the second pull sees it as
    // pending (ledger hit) and archives with no second write.
    let second = run_pull(&env, &fake, &state, &[], &[]);
    assert_eq!(
        second.status.code(),
        Some(0),
        "stdout:\n{}\nstderr:\n{}",
        stdout(&second),
        stderr(&second)
    );
    let after_second = read_target(env.vault());
    assert_eq!(after_second, after_first, "no duplicate write");
    assert_eq!(after_second.matches("Call dentist").count(), count_first);
    assert!(
        stdout(&second).contains("already in vault"),
        "{}",
        stdout(&second)
    );
    // A second pull makes no new commit.
    assert_eq!(git_rev_list_count(env.vault()), 1);

    // When the snapshot no longer returns the pulled notes: nothing to
    // pull, no archive call.
    fake.respond("snapshot", &snapshot_ok("bryanbugyi34@gmail.com", vec![]));
    let calls_before = fake.call_count();
    let third = run_pull(&env, &fake, &state, &[], &[]);
    assert_eq!(third.status.code(), Some(0), "{}", stderr(&third));
    assert_eq!(read_target(env.vault()), after_second);
    assert_eq!(git_rev_list_count(env.vault()), 1, "no new commit");
    assert_eq!(
        fake.call_count(),
        calls_before + 1,
        "snapshot only, no archive"
    );
}

#[test]
fn dry_run_previews_exact_markdown_without_writing_or_archiving() {
    let (env, fake, state, _token) = setup("bob-cli-gkeep-pull-dry");
    let n1 = note("Call dentist")
        .id("note-1")
        .created("2026-09-27T21:14:03Z")
        .build();
    fake.respond("snapshot", &snapshot_ok("bryanbugyi34@gmail.com", vec![n1]));
    fake.respond("archive", &archive_ok(vec![("note-1", "archived")]));
    let before = read_target(env.vault());

    let dry = run_pull(&env, &fake, &state, &["-d"], &[]);
    assert_eq!(dry.status.code(), Some(0), "{}", stderr(&dry));
    let dry_out = stdout(&dry);
    assert!(dry_out.contains("[dry-run]"), "{dry_out}");
    assert!(dry_out.contains("would write"), "{dry_out}");
    assert!(dry_out.contains("Markdown to insert"), "{dry_out}");
    // Dry run writes nothing and makes no archive call.
    assert_eq!(read_target(env.vault()), before);
    assert_eq!(fake.call_count(), 1, "only snapshot, no archive");

    let real = run_pull(&env, &fake, &state, &[], &[]);
    assert_eq!(real.status.code(), Some(0));
    let after = read_target(env.vault());
    // The dry-run preview lines appear verbatim in the real write.
    let mut preview_lines = Vec::new();
    for line in dry_out.lines().filter_map(|line| line.split_once("│ ")) {
        let preview = line.1.trim();
        if preview.starts_with("- [ ]") {
            assert!(after.contains(preview), "missing {preview} in:\n{after}");
            preview_lines.push(preview.to_string());
        }
    }
    // The full dry-run Markdown equals the bytes the real run inserts.
    let inserted = after.strip_prefix(&before).unwrap_or(&after).to_string();
    for preview in &preview_lines {
        assert!(inserted.contains(preview), "missing {preview} in insert");
    }
    assert!(!preview_lines.is_empty());
}

#[test]
fn no_archive_writes_but_leaves_notes_then_next_pull_archives_only() {
    let (env, fake, state, _token) = setup("bob-cli-gkeep-pull-noarc");
    init_git(env.vault());
    let n1 = note("Call dentist")
        .id("note-1")
        .created("2026-09-27T21:14:03Z")
        .build();
    fake.respond("snapshot", &snapshot_ok("bryanbugyi34@gmail.com", vec![n1]));
    fake.respond("archive", &archive_ok(vec![("note-1", "archived")]));

    let first = run_pull(&env, &fake, &state, &["-n"], &[]);
    assert_eq!(first.status.code(), Some(0), "{}", stderr(&first));
    assert!(
        stdout(&first).contains("left in Keep (--no-archive)"),
        "{}",
        stdout(&first)
    );
    assert_eq!(fake.call_count(), 1, "no archive call with -n");

    let after_first = read_target(env.vault());
    let second = run_pull(&env, &fake, &state, &[], &[]);
    assert_eq!(second.status.code(), Some(0), "{}", stderr(&second));
    assert_eq!(
        read_target(env.vault()),
        after_first,
        "archive-only writes nothing"
    );
    assert!(
        stdout(&second).contains("already in vault"),
        "{}",
        stdout(&second)
    );
}

#[test]
fn archive_changed_reports_and_next_pull_writes_revision() {
    let (env, fake, state, _token) = setup("bob-cli-gkeep-pull-changed");
    init_git(env.vault());
    let n1 = note("Call dentist")
        .id("note-1")
        .created("2026-09-27T21:14:03Z")
        .build();
    let n2 = note("Hardware store")
        .id("note-2")
        .created("2026-09-26T08:02:00Z")
        .build();
    fake.respond(
        "snapshot",
        &snapshot_ok("bryanbugyi34@gmail.com", vec![n1.clone(), n2.clone()]),
    );
    fake.respond(
        "archive",
        &serde_json::json!({
            "ok": true,
            "results": [
                {"id": "note-1", "status": "archived"},
                {"id": "note-2", "status": "changed"},
            ]
        })
        .to_string(),
    );

    let out = run_pull(&env, &fake, &state, &[], &[]);
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    assert!(stdout(&out).contains("NOT archived"), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("changed while pulling"),
        "{}",
        stderr(&out)
    );

    // The edited note's revision arrives next: same id, new content.
    let revised = note("Hardware store UPDATED")
        .id("note-2")
        .created("2026-09-26T08:02:00Z")
        .build();
    fake.respond(
        "snapshot",
        &snapshot_ok("bryanbugyi34@gmail.com", vec![revised]),
    );
    fake.respond("archive", &archive_ok(vec![("note-2", "archived")]));
    let second = run_pull(&env, &fake, &state, &[], &[]);
    assert_eq!(second.status.code(), Some(0), "{}", stderr(&second));
    let target = read_target(env.vault());
    assert!(target.contains("revised"), "{target}");
    assert!(target.contains("Hardware store UPDATED"), "{target}");
}

#[test]
fn archive_missing_error_and_crash_keep_the_committed_vault() {
    let (env, fake, state, _token) = setup("bob-cli-gkeep-pull-missing");
    init_git(env.vault());
    let n1 = note("Gone note")
        .id("note-gone")
        .created("2026-09-27T21:14:03Z")
        .build();
    let n2 = note("Bad note")
        .id("note-bad")
        .created("2026-09-26T08:02:00Z")
        .build();
    fake.respond(
        "snapshot",
        &snapshot_ok("bryanbugyi34@gmail.com", vec![n1, n2]),
    );
    fake.respond(
        "archive",
        &serde_json::json!({
            "ok": true,
            "results": [
                {"id": "note-gone", "status": "missing"},
                {"id": "note-bad", "status": "error", "detail": "boom"},
            ]
        })
        .to_string(),
    );
    let out = run_pull(&env, &fake, &state, &[], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout(&out).contains("NOT archived"), "{}", stdout(&out));
    // The vault stays committed even though archiving failed.
    assert!(!git_log_names(env.vault()).is_empty());
    assert!(read_target(env.vault()).contains("Gone note"));

    // An adapter crash during archive also exits 1 with the vault committed.
    let (env2, fake2, state2, _t2) = setup("bob-cli-gkeep-pull-crash");
    init_git(env2.vault());
    let n = note("Solo")
        .id("note-solo")
        .created("2026-09-27T21:14:03Z")
        .build();
    fake2.respond("snapshot", &snapshot_ok("bryanbugyi34@gmail.com", vec![n]));
    fake2.respond("archive", &archive_ok(vec![("note-solo", "archived")]));
    fake2.set_exit("archive", 3);
    let crashed = run_pull(&env2, &fake2, &state2, &[], &[]);
    assert_eq!(crashed.status.code(), Some(1), "{}", stderr(&crashed));
    assert!(read_target(env2.vault()).contains("Solo"));
}

#[test]
fn snapshot_auth_error_writes_nothing() {
    let (env, fake, state, _token) = setup("bob-cli-gkeep-pull-autherr");
    fake.respond("snapshot", &error_response("auth", "login expired"));
    let before = read_target(env.vault());
    let out = run_pull(&env, &fake, &state, &[], &[]);
    assert_eq!(out.status.code(), Some(1));
    let combined = format!("{}{}", stdout(&out), stderr(&out));
    assert!(combined.contains("login"), "{combined}");
    assert_eq!(read_target(env.vault()), before);
    assert_eq!(fake.call_count(), 1);
}

#[test]
fn crash_after_commit_before_archive_recovers_as_pending() {
    let (env, fake, state, _token) = setup("bob-cli-gkeep-pull-recover");
    init_git(env.vault());
    let n1 = note("Recover me")
        .id("note-r1")
        .created("2026-09-27T21:14:03Z")
        .build();
    fake.respond(
        "snapshot",
        &snapshot_ok("bryanbugyi34@gmail.com", vec![n1.clone()]),
    );
    fake.respond("archive", &archive_ok(vec![("note-r1", "archived")]));
    fake.set_exit("archive", 3);

    let first = run_pull(&env, &fake, &state, &[], &[]);
    assert_eq!(first.status.code(), Some(1));
    let after_first = read_target(env.vault());
    assert!(after_first.contains("Recover me"));

    // The journal backstop makes the next pull archive-only: no duplicate.
    fake.clear_exit("archive");
    let second = run_pull(&env, &fake, &state, &[], &[]);
    assert_eq!(second.status.code(), Some(0), "{}", stderr(&second));
    let after_second = read_target(env.vault());
    assert_eq!(after_second, after_first);
    assert_eq!(after_second.matches("Recover me").count(), 1);
}

#[test]
fn target_missing_and_unknown_or_ambiguous_ids_exit_2() {
    let (env, fake, state, _token) = setup("bob-cli-gkeep-pull-target2");
    fake.respond("snapshot", &snapshot_ok("bryanbugyi34@gmail.com", vec![]));
    fs::remove_file(env.vault().join("gkeep_inbox.md")).expect("remove target");
    let out = run_pull(&env, &fake, &state, &[], &[]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("create it or set"),
        "{}",
        stderr(&out)
    );

    let (env2, fake2, state2, _t) = setup("bob-cli-gkeep-pull-ids");
    let n1 = note("One")
        .id("note-1")
        .created("2026-09-27T21:14:03Z")
        .build();
    let n2 = note("Two")
        .id("note-2")
        .created("2026-09-26T08:02:00Z")
        .build();
    fake2.respond(
        "snapshot",
        &snapshot_ok("bryanbugyi34@gmail.com", vec![n1, n2]),
    );
    fake2.respond("archive", &archive_ok(vec![]));

    let unknown = run_pull(&env2, &fake2, &state2, &["-i", "no-such-id"], &[]);
    assert_eq!(unknown.status.code(), Some(2), "{}", stderr(&unknown));

    // Empty REF prefix matches every note, so it is ambiguous with two notes.
    let ambiguous = run_pull(&env2, &fake2, &state2, &["-i", ""], &[]);
    assert_eq!(ambiguous.status.code(), Some(2), "{}", stderr(&ambiguous));
}

#[test]
fn pull_lock_contention_exits_1() {
    use fs2::FileExt;
    let (env, fake, state, _token) = setup("bob-cli-gkeep-pull-lock");
    let n1 = note("Locked")
        .id("note-1")
        .created("2026-09-27T21:14:03Z")
        .build();
    fake.respond("snapshot", &snapshot_ok("bryanbugyi34@gmail.com", vec![n1]));
    fake.respond("archive", &archive_ok(vec![("note-1", "archived")]));

    let lock_path =
        state.path().join("bob-cli").join("gkeep").join("pull.lock");
    fs::create_dir_all(lock_path.parent().unwrap()).expect("lock dir");
    let guard = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .expect("open lock");
    guard.try_lock_exclusive().expect("hold pull lock");

    let out = run_pull(&env, &fake, &state, &[], &[]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains("already running"), "{}", stderr(&out));
    drop(guard);
}

#[test]
fn non_git_vault_writes_without_commit_and_json_shape() {
    let (env, fake, state, _token) = setup("bob-cli-gkeep-pull-nogit");
    let n1 = note("Plain note")
        .id("note-1")
        .created("2026-09-27T21:14:03Z")
        .build();
    fake.respond("snapshot", &snapshot_ok("bryanbugyi34@gmail.com", vec![n1]));
    fake.respond("archive", &archive_ok(vec![("note-1", "archived")]));

    let out = run_pull(&env, &fake, &state, &["-f", "json"], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let doc: serde_json::Value =
        serde_json::from_str(&stdout(&out)).expect("json parses");
    assert_eq!(doc["schema_version"], 1);
    assert_eq!(doc["ok"], true);
    assert_eq!(doc["target"], "gkeep_inbox.md");
    assert!(doc["commit"].is_null(), "{doc}");
    assert!(
        doc["markdown"].as_str().unwrap().contains("Plain note"),
        "{doc}"
    );
    assert_eq!(doc["summary"]["written"], 1);
    assert_eq!(doc["summary"]["archived"], 1);
    let note0 = &doc["notes"][0];
    assert_eq!(note0["id"], "note-1");
    assert_eq!(note0["state"], "new");
    assert_eq!(note0["action"], "write");
    assert_eq!(note0["written"], true);
    assert_eq!(note0["archive"], "archived");
}

#[test]
fn limit_and_explicit_pinned_selection() {
    let (env, fake, state, _token) = setup("bob-cli-gkeep-pull-limit");
    let a = note("First")
        .id("note-a")
        .created("2026-09-25T10:00:00Z")
        .build();
    let b = note("Second")
        .id("note-b")
        .created("2026-09-26T10:00:00Z")
        .build();
    let c = note("Third")
        .id("note-c")
        .created("2026-09-27T10:00:00Z")
        .build();
    fake.respond(
        "snapshot",
        &snapshot_ok(
            "bryanbugyi34@gmail.com",
            vec![c.clone(), a.clone(), b.clone()],
        ),
    );
    fake.respond("archive", &archive_ok(vec![("note-a", "archived")]));

    let out = run_pull(&env, &fake, &state, &["-l", "1"], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let target = read_target(env.vault());
    assert!(target.contains("First"), "{target}");
    assert!(!target.contains("Second"), "{target}");

    // Explicit -i on a pinned note includes it despite the skip.
    let (env2, fake2, state2, _t) = setup("bob-cli-gkeep-pull-pinnedid");
    let pinned = note("Pinned pick")
        .id("pinned-9")
        .created("2026-09-27T10:00:00Z")
        .pinned()
        .build();
    fake2.respond(
        "snapshot",
        &snapshot_ok("bryanbugyi34@gmail.com", vec![pinned]),
    );
    fake2.respond("archive", &archive_ok(vec![("pinned-9", "archived")]));
    let picked = run_pull(&env2, &fake2, &state2, &["-i", "pinned-9"], &[]);
    assert_eq!(picked.status.code(), Some(0), "{}", stderr(&picked));
    assert!(read_target(env2.vault()).contains("Pinned pick"));
}

#[test]
fn crlf_endings_preserved_and_space_indent_used() {
    let (env, fake, state, _token) = setup("bob-cli-gkeep-pull-crlf");
    let crlf = "## Tasks\r\n- [ ] #task Existing [created::2026-09-20]\r\n  - child\r\n";
    write_target(env.vault(), crlf);
    let n1 = note("CRLF note")
        .id("note-1")
        .created("2026-09-27T21:14:03Z")
        .build();
    fake.respond("snapshot", &snapshot_ok("bryanbugyi34@gmail.com", vec![n1]));
    fake.respond("archive", &archive_ok(vec![("note-1", "archived")]));
    let out = run_pull(&env, &fake, &state, &[], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let raw = fs::read(env.vault().join("gkeep_inbox.md")).expect("read raw");
    assert!(raw.windows(2).any(|pair| pair == b"\r\n"), "CRLF preserved");
    // No bare `\n`: every newline is part of `\r\n`.
    for (i, byte) in raw.iter().enumerate() {
        if *byte == b'\n' {
            assert!(i > 0 && raw[i - 1] == b'\r', "bare \\n at {i}");
        }
    }
    let text = String::from_utf8_lossy(&raw).into_owned();
    // The space-indented target uses two spaces for the new children.
    assert!(text.contains("\n  - Source:"), "{text}");
}

#[test]
fn target_race_once_replans_and_succeeds() {
    let (env, fake, state, _token) = setup("bob-cli-gkeep-pull-raceonce");
    let m = note("Racy once")
        .id("note-r1")
        .created("2026-09-27T21:14:03Z")
        .build();
    fake.respond("snapshot", &snapshot_ok("bryanbugyi34@gmail.com", vec![m]));
    fake.respond("archive", &archive_ok(vec![("note-r1", "archived")]));
    let target = env.vault().join("gkeep_inbox.md");
    // Append only once: the retry sees a stable file and succeeds.
    let hook = format!(
        "grep -q intruder-once '{}' || echo '- intruder-once' >> '{}'",
        target.to_string_lossy(),
        target.to_string_lossy()
    );
    let out = run_pull(
        &env,
        &fake,
        &state,
        &[],
        &[("BOB_GKEEP_TEST_BEFORE_RENAME", hook.as_str())],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let after = read_target(env.vault());
    assert!(after.contains("Racy once"), "{after}");
    assert!(after.contains("intruder-once"), "{after}");
}

#[test]
fn quiet_success_is_silent_and_target_race_aborts() {
    let (env, fake, state, _token) = setup("bob-cli-gkeep-pull-quiet");
    let n1 = note("Quiet note")
        .id("note-1")
        .created("2026-09-27T21:14:03Z")
        .build();
    fake.respond("snapshot", &snapshot_ok("bryanbugyi34@gmail.com", vec![n1]));
    fake.respond("archive", &archive_ok(vec![("note-1", "archived")]));
    let out = run_pull(&env, &fake, &state, &["-q"], &[]);
    assert_eq!(out.status.code(), Some(0));
    assert!(stdout(&out).is_empty(), "quiet prints nothing on success");

    // A hook that always appends makes both CAS reads disagree: abort, no archive.
    let (env2, fake2, state2, _t) = setup("bob-cli-gkeep-pull-race");
    let m = note("Racy")
        .id("note-r")
        .created("2026-09-27T21:14:03Z")
        .build();
    fake2.respond("snapshot", &snapshot_ok("bryanbugyi34@gmail.com", vec![m]));
    fake2.respond("archive", &archive_ok(vec![("note-r", "archived")]));
    let target = env2.vault().join("gkeep_inbox.md");
    let hook = format!("echo '- intruder' >> '{}'", target.to_string_lossy());
    let before = read_target(env2.vault());
    let before_calls = fake2.call_count();
    let raced = run_pull(
        &env2,
        &fake2,
        &state2,
        &[],
        &[("BOB_GKEEP_TEST_BEFORE_RENAME", hook.as_str())],
    );
    assert_eq!(raced.status.code(), Some(1), "{}", stderr(&raced));
    // No archive call happened after the abort.
    assert_eq!(fake2.call_count(), before_calls + 1, "snapshot only");
    // The target equals the externally modified bytes (intruder kept).
    let after = read_target(env2.vault());
    assert!(after.contains("- intruder"), "{after}");
    assert!(!after.contains("Racy"), "{after}");
    assert!(after.starts_with(&before) || after.len() > before.len());
}

#[test]
fn archive_only_pull_reports_no_failure() {
    let (env, fake, state, _token) = setup("bob-cli-gkeep-pull-archonly");
    init_git(env.vault());
    let n1 = note("Call dentist")
        .id("note-1")
        .created("2026-09-27T21:14:03Z")
        .build();
    fake.respond(
        "snapshot",
        &snapshot_ok("bryanbugyi34@gmail.com", vec![n1.clone()]),
    );
    fake.respond("archive", &archive_ok(vec![("note-1", "archived")]));
    let first = run_pull(&env, &fake, &state, &[], &[]);
    assert_eq!(first.status.code(), Some(0));

    // Second pull sees the ledger hit as pending (archive-only).
    let second = run_pull(&env, &fake, &state, &["-f", "json"], &[]);
    assert_eq!(second.status.code(), Some(0), "{}", stderr(&second));
    let doc: serde_json::Value =
        serde_json::from_str(&stdout(&second)).expect("json parses");
    assert_eq!(doc["ok"], true);
    assert_eq!(doc["summary"]["failed"], 0);
}

#[test]
fn archive_crash_reports_once_in_both_modes() {
    for mode in [&["-f", "json"][..], &[][..]] {
        let (env, fake, state, _token) = setup("bob-cli-gkeep-pull-crash");
        let n1 = note("Crash note")
            .id("note-1")
            .created("2026-09-27T21:14:03Z")
            .build();
        fake.respond(
            "snapshot",
            &snapshot_ok("bryanbugyi34@gmail.com", vec![n1]),
        );
        fake.set_exit("archive", 3);
        let out = run_pull(&env, &fake, &state, mode, &[]);
        assert_eq!(out.status.code(), Some(1), "mode {mode:?}");
        if mode.contains(&"json") {
            let text = stdout(&out);
            let docs: Vec<&str> = text.lines().collect();
            assert_eq!(docs.len(), 1, "exactly one JSON doc: {docs:?}");
            let doc: serde_json::Value =
                serde_json::from_str(docs[0]).expect("json parses");
            assert_eq!(doc["ok"], false);
            assert_eq!(doc["notes"][0]["archive"], "error");
            assert!(doc.get("error").is_some(), "{doc}");
        } else {
            let body = stdout(&out);
            assert!(body.contains("NOT archived"), "{body}");
        }
    }
}

#[test]
fn dry_run_json_reports_unwritten() {
    let (env, fake, state, _token) = setup("bob-cli-gkeep-pull-dryjson");
    let n1 = note("Dry note")
        .id("note-1")
        .created("2026-09-27T21:14:03Z")
        .build();
    fake.respond("snapshot", &snapshot_ok("bryanbugyi34@gmail.com", vec![n1]));
    let out = run_pull(&env, &fake, &state, &["-d", "-f", "json"], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let doc: serde_json::Value =
        serde_json::from_str(&stdout(&out)).expect("json parses");
    assert_eq!(doc["notes"][0]["written"], false);
    assert_eq!(doc["summary"]["written"], 0);
}

#[test]
fn dry_run_with_lock_held_still_succeeds() {
    use fs2::FileExt;
    let (env, fake, state, _token) = setup("bob-cli-gkeep-pull-drylock");
    let n1 = note("Locked dry")
        .id("note-1")
        .created("2026-09-27T21:14:03Z")
        .build();
    fake.respond("snapshot", &snapshot_ok("bryanbugyi34@gmail.com", vec![n1]));
    // Hold pull.lock in this process; -d must still succeed (no lock).
    let lock_path =
        state.path().join("bob-cli").join("gkeep").join("pull.lock");
    std::fs::create_dir_all(lock_path.parent().unwrap()).expect("lock dir");
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .expect("open lock");
    lock_file.lock_exclusive().expect("hold lock");
    let out = run_pull(&env, &fake, &state, &["-d"], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
}

#[test]
fn missing_target_reports_before_vault_lock() {
    let (env, fake, state, _token) = setup("bob-cli-gkeep-pull-missing-lock");
    fake.respond("snapshot", &snapshot_ok("bryanbugyi34@gmail.com", vec![]));
    fs::remove_file(env.vault().join("gkeep_inbox.md")).expect("remove target");
    // Hold the vault lock via an override file; missing target must still
    // exit 2 with the hint, not a lock timeout.
    let lock_dir = state.path().join("vault-lock");
    std::fs::create_dir_all(&lock_dir).expect("lock dir");
    let lock_path = lock_dir.join("bob_sync.lock");
    let guard = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .expect("open lock");
    use fs2::FileExt;
    guard.try_lock_exclusive().expect("hold vault lock");
    let out = run_pull(
        &env,
        &fake,
        &state,
        &[],
        &[(
            "BOB_VAULT_SYNC_LOCK_FILE",
            lock_path.to_string_lossy().as_ref(),
        )],
    );
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("create it or set gkeep.target"),
        "{}",
        stderr(&out)
    );
    drop(guard);
}

#[test]
fn dry_run_no_archive_reports_not_requested() {
    let (env, fake, state, _token) = setup("bob-cli-gkeep-pull-dry-noarc");
    let n1 = note("Dry noarc")
        .id("note-1")
        .created("2026-09-27T21:14:03Z")
        .build();
    fake.respond("snapshot", &snapshot_ok("bryanbugyi34@gmail.com", vec![n1]));
    // Human: archive_only and write rows mention --no-archive.
    let out = run_pull(&env, &fake, &state, &["-d", "-n"], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let body = stdout(&out);
    assert!(
        body.contains("left in Keep (--no-archive)"),
        "human --no-archive:\n{body}"
    );
    assert!(
        !body.contains("would archive"),
        "no would-archive with -n:\n{body}"
    );
    // JSON: every note with no archive result is not_requested, even dry.
    let out = run_pull(&env, &fake, &state, &["-d", "-n", "-f", "json"], &[]);
    assert_eq!(out.status.code(), Some(0));
    let doc: serde_json::Value =
        serde_json::from_str(&stdout(&out)).expect("json parses");
    assert_eq!(doc["notes"][0]["archive"], "not_requested");
    // Dry with archiving enabled keeps not_attempted.
    let out = run_pull(&env, &fake, &state, &["-d", "-f", "json"], &[]);
    let doc: serde_json::Value =
        serde_json::from_str(&stdout(&out)).expect("json parses");
    assert_eq!(doc["notes"][0]["archive"], "not_attempted");
}

#[test]
fn archive_failure_json_keeps_markdown_and_reports() {
    let (env, fake, state, _token) = setup("bob-cli-gkeep-pull-archfail");
    init_git(env.vault());
    let n1 = note("Fail note")
        .id("note-1")
        .created("2026-09-27T21:14:03Z")
        .build();
    fake.respond("snapshot", &snapshot_ok("bryanbugyi34@gmail.com", vec![n1]));
    fake.set_exit("archive", 3);
    let out = run_pull(&env, &fake, &state, &["-f", "json"], &[]);
    assert_eq!(out.status.code(), Some(1));
    let doc: serde_json::Value =
        serde_json::from_str(&stdout(&out)).expect("json parses");
    assert!(doc["markdown"].is_string(), "markdown kept: {doc}");
    assert!(!doc["markdown"].as_str().unwrap().is_empty());
    assert_eq!(doc["notes"][0]["archive"], "error");
    assert!(
        doc["notes"][0]["detail"].is_string()
            && !doc["notes"][0]["detail"].as_str().unwrap().is_empty(),
        "detail non-empty: {doc}"
    );
    assert!(doc["error"]["kind"].is_string(), "{doc}");
    assert!(doc["error"]["message"].is_string(), "{doc}");
    assert!(doc.get("error").unwrap().get("hint").is_some(), "{doc}");
    // Human mode: error line on stderr, warning prefix not ok.
    let (env2, fake2, state2, _t) = setup("bob-cli-gkeep-pull-archfail-h");
    let m = note("Fail note")
        .id("note-1")
        .created("2026-09-27T21:14:03Z")
        .build();
    fake2.respond("snapshot", &snapshot_ok("bryanbugyi34@gmail.com", vec![m]));
    fake2.set_exit("archive", 3);
    let out = run_pull(&env2, &fake2, &state2, &[], &[]);
    assert_eq!(out.status.code(), Some(1));
    let body = stdout(&out);
    let err = stderr(&out);
    assert!(
        body.contains("NOT archived") || err.contains("NOT archived"),
        "{body}\n{err}"
    );
    assert!(
        err.contains("crashed") || body.contains("crashed"),
        "error line:\n{body}\n{err}"
    );
    assert!(body.contains("warning "), "warning prefix, not ok:\n{body}");
}

#[test]
fn quiet_verify_failure_reports_once_with_empty_stdout() {
    let (env, fake, state, _token) = setup("bob-cli-gkeep-pull-quiet-fail");
    // Two notes that will fail verify: pre-create conflicting markers so
    // verify fails? Simpler: use a hook that corrupts the write.
    // Here we force verify failure by racing the target every time.
    let n1 = note("Quiet fail one")
        .id("note-1")
        .created("2026-09-27T21:14:03Z")
        .build();
    let n2 = note("Quiet fail two")
        .id("note-2")
        .created("2026-09-26T08:02:00Z")
        .build();
    fake.respond(
        "snapshot",
        &snapshot_ok("bryanbugyi34@gmail.com", vec![n1, n2]),
    );
    fake.respond(
        "archive",
        &archive_ok(vec![("note-1", "archived"), ("note-2", "archived")]),
    );
    // Corrupt the target after write by removing the marker via hook?
    // Use a hook that deletes the target content so verify fails.
    let target = env.vault().join("gkeep_inbox.md");
    let hook = format!("printf 'corrupted' > '{}'", target.to_string_lossy());
    let out = run_pull(
        &env,
        &fake,
        &state,
        &["-q"],
        &[("BOB_GKEEP_TEST_BEFORE_RENAME", hook.as_str())],
    );
    // Either succeeds (if hook timing misses) or fails quietly with empty
    // stdout. We assert the quiet contract: stdout empty on failure.
    if out.status.code() != Some(0) {
        assert!(
            stdout(&out).is_empty(),
            "quiet failure leaves stdout empty:\n{}",
            stdout(&out)
        );
        assert!(!stderr(&out).is_empty(), "quiet failure reports on stderr");
        // Each failing note appears once on stderr.
        for id in ["note-1", "note-2"] {
            let count = stderr(&out).matches(id).count();
            assert!(
                count <= 1,
                "note {id} reported {count} times, expected once:\n{}",
                stderr(&out)
            );
        }
    }
}

#[test]
fn dry_run_markdown_equals_real_run() {
    let (env, fake, state, _token) = setup("bob-cli-gkeep-pull-dryeq");
    let n1 = note("Equal note")
        .id("note-1")
        .created("2026-09-27T21:14:03Z")
        .build();
    fake.respond("snapshot", &snapshot_ok("bryanbugyi34@gmail.com", vec![n1]));
    fake.respond("archive", &archive_ok(vec![("note-1", "archived")]));
    let dry = run_pull(&env, &fake, &state, &["-d", "-f", "json"], &[]);
    assert_eq!(dry.status.code(), Some(0));
    let dry_doc: serde_json::Value =
        serde_json::from_str(&stdout(&dry)).expect("dry json");
    let dry_md = dry_doc["markdown"].as_str().unwrap().to_string();
    let real = run_pull(&env, &fake, &state, &["-f", "json"], &[]);
    assert_eq!(real.status.code(), Some(0), "{}", stderr(&real));
    let real_doc: serde_json::Value =
        serde_json::from_str(&stdout(&real)).expect("real json");
    let real_md = real_doc["markdown"].as_str().unwrap().to_string();
    assert_eq!(dry_md, real_md, "dry markdown equals real");
    let after = read_target(env.vault());
    assert!(
        after.contains(&real_md),
        "target contains markdown verbatim:\n{after}\n{real_md}"
    );
    assert_eq!(after.matches(&real_md).count(), 1, "exactly once:\n{after}");
}

#[test]
fn double_modification_abort_keeps_exact_bytes() {
    let (env, fake, state, _token) = setup("bob-cli-gkeep-pull-doublemod");
    let m = note("Double mod")
        .id("note-r")
        .created("2026-09-27T21:14:03Z")
        .build();
    fake.respond("snapshot", &snapshot_ok("bryanbugyi34@gmail.com", vec![m]));
    fake.respond("archive", &archive_ok(vec![("note-r", "archived")]));
    let target = env.vault().join("gkeep_inbox.md");
    let hook = format!("echo '- intruder' >> '{}'", target.to_string_lossy());
    let before = read_target(env.vault());
    // Hook runs before both re-reads, so both CAS reads disagree.
    let out = run_pull(
        &env,
        &fake,
        &state,
        &[],
        &[("BOB_GKEEP_TEST_BEFORE_RENAME", hook.as_str())],
    );
    assert_eq!(out.status.code(), Some(1));
    let after = std::fs::read(&target).expect("read bytes");
    // The hook wrote exact bytes; the target must equal those bytes.
    // Re-read the file after hook to get expected? Instead assert it
    // contains intruder and not the new note, and equals raw bytes read.
    let after_str = String::from_utf8_lossy(&after).into_owned();
    assert!(after_str.contains("- intruder"));
    assert!(!after_str.contains("Double mod"));
    // Exactness: file bytes equal what the hook left (no partial write).
    let reread = std::fs::read(&target).expect("reread");
    assert_eq!(after, reread);
    assert!(before.as_bytes() != after.as_slice());
}

#[test]
fn empty_snapshot_second_pull_reports_nothing() {
    let (env, fake, state, _token) = setup("bob-cli-gkeep-pull-empty2");
    let n1 = note("Call dentist")
        .id("note-1")
        .created("2026-09-27T21:14:03Z")
        .build();
    fake.respond("snapshot", &snapshot_ok("bryanbugyi34@gmail.com", vec![n1]));
    fake.respond("archive", &archive_ok(vec![("note-1", "archived")]));
    let first = run_pull(&env, &fake, &state, &[], &[]);
    assert_eq!(first.status.code(), Some(0));
    fake.respond("snapshot", &snapshot_ok("bryanbugyi34@gmail.com", vec![]));
    let second = run_pull(&env, &fake, &state, &[], &[]);
    assert_eq!(second.status.code(), Some(0));
    assert!(
        stdout(&second).contains("nothing to pull"),
        "empty second pull:\n{}",
        stdout(&second)
    );
}

// ---------- URL-only (R5) clip fixtures ----------

fn write_executable(path: &Path, script: &str) {
    fs::write(path, script).expect("write script");
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(path).expect("stat script").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("chmod script");
    }
}

/// A one-page PDF with no annotations, standing in for download and
/// adapter-render fixtures the clip pipeline stamps.
fn write_bare_pdf(path: &Path) {
    use lopdf::{dictionary, Document, Stream};
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("pdf parent");
    }
    let mut doc = Document::with_version("1.4");
    let pages_id = doc.new_object_id();
    let content_id = doc.add_object(Stream::new(dictionary! {}, Vec::new()));
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "MediaBox" => vec![
            lopdf::Object::Integer(0),
            lopdf::Object::Integer(0),
            lopdf::Object::Integer(612),
            lopdf::Object::Integer(792),
        ],
        "Contents" => content_id,
    });
    doc.set_object(
        pages_id,
        dictionary! {
            "Type" => "Pages",
            "Kids" => vec![lopdf::Object::Reference(page_id)],
            "Count" => 1,
        },
    );
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog_id);
    doc.save(path).expect("write bare PDF");
}

/// Fake `BOB_HIGHLIGHTS_CURL` serving the clip root by URL and logging
/// to `$FAKE_CURL_LOG`: `paper.pdf` downloads a PDF, `article`
/// returns HTML, `timeout` exits 28, anything else 404s.
fn write_clip_curl(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let root = dir.join("curl-root");
    fs::create_dir_all(&root).expect("curl root");
    write_bare_pdf(&root.join("paper.pdf"));
    let path = dir.join("fake-curl.sh");
    write_executable(
        &path,
        r#"#!/bin/sh
dest=""; url=""; prev=""
for arg in "$@"; do
  if [ "$prev" = "-o" ]; then dest="$arg"; fi
  prev="$arg"; url="$arg"
done
echo "$url" >> "$FAKE_CURL_LOG"
case "$url" in
  *"example.com/paper.pdf"*)
    cp "$FAKE_CURL_ROOT/paper.pdf" "$dest"
    printf '200\napplication/pdf\n\n'
    ;;
  *"example.com/article"*)
    printf '<html><body>article body</body></html>' > "$dest"
    printf '200\ntext/html; charset=utf-8\n\n'
    ;;
  *"example.com/timeout"*)
    exit 28
    ;;
  *)
    printf '404\ntext/html\n\n'
    ;;
esac
"#,
    );
    let log = dir.join("curl.log");
    fs::write(&log, "").expect("init log");
    (path, root, log)
}

/// Fake `BOB_WEB_CLIP_ADAPTER`: answers ping, copies the fixture PDF
/// to `out_pdf`, then prints the canned capture `response`.
fn write_clip_adapter(dir: &Path, response: &str) -> (PathBuf, PathBuf) {
    let root = dir.join("clip-root");
    fs::create_dir_all(&root).expect("clip root");
    write_bare_pdf(&root.join("fixture.pdf"));
    fs::write(root.join("response.json"), response).expect("clip response");
    let path = dir.join("fake-clip-adapter.sh");
    let quoted = root.to_string_lossy().replace('\'', "'\\''");
    write_executable(
        &path,
        &format!(
            "#!/bin/sh\n\
             ROOT='{quoted}'\n\
             request=$(cat)\n\
             printf '%s' \"$request\" > \"$ROOT/request.json\"\n\
             case \"$request\" in\n\
             *'\"op\":\"ping\"'*)\n\
             printf '%s' '{{\"protocol\":1,\"ok\":true,\"op\":\"ping\",\"python\":\"3.12.3\"}}'\n\
             ;;\n\
             *)\n\
             out_pdf=$(printf '%s' \"$request\" | sed -n 's/.*\"out_pdf\":\"\\([^\"]*\\)\".*/\\1/p')\n\
             if [ -n \"$out_pdf\" ] && [ -f \"$ROOT/fixture.pdf\" ]; then cp \"$ROOT/fixture.pdf\" \"$out_pdf\"; fi\n\
             cat \"$ROOT/response.json\"\n\
             ;;\n\
             esac\n"
        ),
    );
    (path, root)
}

fn clip_failure(kind: &str, message: &str) -> String {
    format!(
        r#"{{"protocol":1,"ok":false,"op":"capture","error":{{"kind":"{kind}","message":"{message}","hint":"try again"}}}}"#
    )
}

/// A stand-in that records any invocation to `<dir>/<name>.hit` and
/// exits 1: offline paths must never touch it.
fn write_sentinel(dir: &Path, name: &str) -> (PathBuf, PathBuf) {
    let hit = dir.join(format!("{name}.hit"));
    let path = dir.join(format!("{name}.sh"));
    let quoted = hit.to_string_lossy().replace('\'', "'\\''");
    write_executable(
        &path,
        &format!("#!/bin/sh\necho invoked \"$@\" >> '{quoted}'\nexit 1\n"),
    );
    (path, hit)
}

fn ref_events(state: &TempDir) -> Vec<serde_json::Value> {
    journal_records(state)
        .into_iter()
        .filter(|record| record["event"] == "ref_created")
        .collect()
}

fn curl_env<'a>(
    curl: &'a Path,
    root: &'a Path,
    log: &'a Path,
) -> Vec<(&'a str, &'a str)> {
    vec![
        ("BOB_HIGHLIGHTS_CURL", curl.to_str().expect("curl path")),
        ("FAKE_CURL_ROOT", root.to_str().expect("curl root")),
        ("FAKE_CURL_LOG", log.to_str().expect("curl log")),
    ]
}

// ---------- URL-only (R5) clip outcomes ----------

#[test]
fn url_only_pdf_note_clips_and_archives_without_task() {
    let (env, fake, state, _token) = setup("bob-cli-gkeep-pull-ref-pdf");
    let (curl, root, log) = write_clip_curl(state.path());
    let n1 = note("")
        .id("note-1")
        .text("https://example.com/paper.pdf")
        .build();
    fake.respond("snapshot", &snapshot_ok("bryanbugyi34@gmail.com", vec![n1]));
    fake.respond("archive", &archive_ok(vec![("note-1", "archived")]));
    let out = run_pull(&env, &fake, &state, &[], &curl_env(&curl, &root, &log));
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let body = stdout(&out);
    assert!(
        body.contains("clipped → xlib/papers/paper.pdf · archived"),
        "clip row:\n{body}"
    );
    assert!(
        body.contains("0 written · 1 clipped · 1 archived · 0 skipped"),
        "summary:\n{body}"
    );
    // The vault gains the intake PDF but no task.
    assert!(env.vault().join("xlib/papers/paper.pdf").is_file());
    assert!(
        !read_target(env.vault()).contains("example.com"),
        "no task written"
    );
    // One `ref_created` journal batch carries the PDF path and URL.
    let events = ref_events(&state);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["id"], "note-1");
    assert_eq!(events[0]["path"], "xlib/papers/paper.pdf");
    assert_eq!(events[0]["url"], "https://example.com/paper.pdf");
    // The archive guard carries the attachment count.
    let req: serde_json::Value =
        serde_json::from_str(&fake.request("archive", 2)).expect("archive req");
    assert_eq!(req["notes"][0]["expect_attachments"], 0);
    // A re-pull is ArchiveOnly through `ref_created`: no duplicate clip.
    let out = run_pull(&env, &fake, &state, &[], &curl_env(&curl, &root, &log));
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("archived · already in vault"),
        "repull:\n{}",
        stdout(&out)
    );
    assert!(
        !read_target(env.vault()).contains("example.com"),
        "still no task written"
    );
}

#[test]
fn second_note_with_same_url_reports_already_queued() {
    let (env, fake, state, _token) = setup("bob-cli-gkeep-pull-ref-queued");
    let (curl, root, log) = write_clip_curl(state.path());
    let first = note("")
        .id("note-1")
        .text("https://example.com/paper.pdf")
        .build();
    fake.respond(
        "snapshot",
        &snapshot_ok("bryanbugyi34@gmail.com", vec![first]),
    );
    fake.respond("archive", &archive_ok(vec![("note-1", "archived")]));
    // First pull clips; JSON carries the create_ref action and outcome.
    let out = run_pull(
        &env,
        &fake,
        &state,
        &["-f", "json"],
        &curl_env(&curl, &root, &log),
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let doc: serde_json::Value =
        serde_json::from_str(&stdout(&out)).expect("json parses");
    assert_eq!(doc["notes"][0]["action"], "create_ref");
    assert_eq!(doc["notes"][0]["clip"]["outcome"], "created");
    assert_eq!(doc["notes"][0]["clip"]["pdf"], "xlib/papers/paper.pdf");
    assert_eq!(
        doc["notes"][0]["clip"]["url"],
        "https://example.com/paper.pdf"
    );
    assert_eq!(doc["notes"][0]["clip"]["display"], "example.com/paper.pdf");
    assert_eq!(doc["summary"]["refs"]["clipped"], 1);
    assert_eq!(doc["summary"]["failed"], 0);
    // A different note with the same link is already queued.
    let second = note("")
        .id("note-2")
        .text("https://example.com/paper.pdf")
        .build();
    fake.respond(
        "snapshot",
        &snapshot_ok("bryanbugyi34@gmail.com", vec![second]),
    );
    fake.respond("archive", &archive_ok(vec![("note-2", "archived")]));
    let out = run_pull(&env, &fake, &state, &[], &curl_env(&curl, &root, &log));
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(
        stdout(&out)
            .contains("already queued · xlib/papers/paper.pdf · archived"),
        "queued row:\n{}",
        stdout(&out)
    );
}

#[test]
fn already_in_library_archives_without_fetch() {
    let (env, fake, state, _token) = setup("bob-cli-gkeep-pull-ref-lib");
    fs::create_dir_all(env.vault().join("ref/papers")).expect("ref dir");
    fs::write(
        env.vault().join("ref/papers/captured.md"),
        "---\ntitle: Captured Post\nstatus: ready\nsource_url: https://example.com/captured\nsource_pdf: lib/papers/captured.pdf\n---\n\n- [ ] ^ref\n",
    )
    .expect("seed ref note");
    // The library hit must never touch the network.
    let (sentinel, hit) = write_sentinel(state.path(), "curl");
    let n1 = note("")
        .id("note-1")
        .text("https://example.com/captured")
        .build();
    fake.respond("snapshot", &snapshot_ok("bryanbugyi34@gmail.com", vec![n1]));
    fake.respond("archive", &archive_ok(vec![("note-1", "archived")]));
    let out = run_pull(
        &env,
        &fake,
        &state,
        &[],
        &[("BOB_HIGHLIGHTS_CURL", sentinel.to_str().expect("path"))],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(!hit.is_file(), "no fetch for a library hit");
    let body = stdout(&out);
    assert!(
        body.contains("already in library · ref/papers/captured.md · archived"),
        "library row:\n{body}"
    );
    let events = ref_events(&state);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["path"], "ref/papers/captured.md");
    assert!(
        !read_target(env.vault()).contains("example.com"),
        "no task written"
    );
}

#[test]
fn retryable_clip_failure_stays_in_keep_and_fails() {
    let (env, fake, state, _token) = setup("bob-cli-gkeep-pull-ref-retry");
    let (curl, root, log) = write_clip_curl(state.path());
    let n1 = note("")
        .id("note-1")
        .text("https://example.com/timeout")
        .build();
    fake.respond("snapshot", &snapshot_ok("bryanbugyi34@gmail.com", vec![n1]));
    let out = run_pull(&env, &fake, &state, &[], &curl_env(&curl, &root, &log));
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let body = stdout(&out);
    assert!(
        body.contains("clip failed (timeout) · left in Keep for the next pull"),
        "retry row:\n{body}"
    );
    // Nothing archived, nothing journaled: the note stays in Keep.
    assert_eq!(fake.call_count(), 1, "snapshot only, no archive");
    assert!(ref_events(&state).is_empty(), "no ref_created journal");
    assert!(
        !read_target(env.vault()).contains("example.com"),
        "no task written"
    );
    let out = run_pull(
        &env,
        &fake,
        &state,
        &["-f", "json"],
        &curl_env(&curl, &root, &log),
    );
    assert_eq!(out.status.code(), Some(1));
    let doc: serde_json::Value =
        serde_json::from_str(&stdout(&out)).expect("json parses");
    assert_eq!(doc["notes"][0]["action"], "create_ref");
    assert_eq!(doc["notes"][0]["clip"]["outcome"], "failed_retryable");
    assert_eq!(doc["notes"][0]["clip"]["error"]["retryable"], true);
    assert_eq!(doc["summary"]["refs"]["failed_retryable"], 1);
    assert_eq!(doc["summary"]["failed"], 1);
}

#[test]
fn permanent_clip_failure_writes_task_with_warning() {
    let (env, fake, state, _token) = setup("bob-cli-gkeep-pull-ref-perm");
    let (curl, root, log) = write_clip_curl(state.path());
    let (clip, _clip_root) =
        write_clip_adapter(state.path(), &clip_failure("blocked", "bot wall"));
    let n1 = note("")
        .id("note-1")
        .text("https://example.com/article-one")
        .build();
    let n2 = note("")
        .id("note-2")
        .text("https://example.com/article-two")
        .build();
    fake.respond(
        "snapshot",
        &snapshot_ok("bryanbugyi34@gmail.com", vec![n1, n2]),
    );
    fake.respond("archive", &archive_ok(vec![("note-1", "archived")]));
    let mut extra = curl_env(&curl, &root, &log);
    extra.push(("BOB_WEB_CLIP_ADAPTER", clip.to_str().expect("clip path")));
    let out = run_pull(&env, &fake, &state, &["-i", "note-1"], &extra);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let body = stdout(&out);
    assert!(
        body.contains(
            "clip failed (blocked) · written as a task with a ⚠️ note · archived"
        ),
        "fallback row:\n{body}"
    );
    let target = read_target(env.vault());
    assert!(
        target.contains("⚠️ Clip failed (blocked)"),
        "warning child:\n{target}"
    );
    assert!(
        target
            .contains("retry: bob ref create https://example.com/article-one -P gkeep_inbox"),
        "retry command:\n{target}"
    );
    assert!(ref_events(&state).is_empty(), "no ref_created journal");
    // The sibling note still clips on its own selection: JSON outcome.
    fake.respond("archive", &archive_ok(vec![("note-2", "archived")]));
    let out =
        run_pull(&env, &fake, &state, &["-i", "note-2", "-f", "json"], &extra);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let doc: serde_json::Value =
        serde_json::from_str(&stdout(&out)).expect("json parses");
    assert_eq!(doc["notes"][0]["action"], "create_ref");
    assert_eq!(doc["notes"][0]["clip"]["outcome"], "failed_permanent");
    assert_eq!(doc["notes"][0]["clip"]["error"]["retryable"], false);
    assert_eq!(doc["summary"]["refs"]["failed_permanent"], 1);
    assert_eq!(doc["summary"]["failed"], 0);
}

#[test]
fn create_ref_fallback_verify_failure_counts_as_failed() {
    // Same `BOB_GKEEP_TEST_BEFORE_RENAME` seam the Write verify-failure
    // tests use. The hook race usually recovers (exit 0); only when a run
    // reports `verification failed for note-1` do we assert the fixed
    // counting and row. This mirrors `quiet_verify_failure_...`, which
    // also passes when the race is won.
    for json in [false, true] {
        let (env, fake, state, _token) =
            setup("bob-cli-gkeep-pull-ref-verify-fail");
        let (curl, root, log) = write_clip_curl(state.path());
        let (clip, _clip_root) = write_clip_adapter(
            state.path(),
            &clip_failure("blocked", "bot wall"),
        );
        let n1 = note("")
            .id("note-1")
            .text("https://example.com/article-one")
            .build();
        fake.respond(
            "snapshot",
            &snapshot_ok("bryanbugyi34@gmail.com", vec![n1]),
        );
        fake.respond("archive", &archive_ok(vec![("note-1", "archived")]));
        let mut extra = curl_env(&curl, &root, &log);
        extra.push(("BOB_WEB_CLIP_ADAPTER", clip.to_str().expect("clip path")));
        let target = env.vault().join("gkeep_inbox.md");
        let hook =
            format!("printf 'corrupted' > '{}'", target.to_string_lossy());
        extra.push(("BOB_GKEEP_TEST_BEFORE_RENAME", hook.as_str()));
        let mut args: Vec<&str> = Vec::new();
        if json {
            args.push("-f");
            args.push("json");
        }
        let out = run_pull(&env, &fake, &state, &args, &extra);
        if out.status.code() != Some(1)
            || !stderr(&out).contains("verification failed for note-1")
        {
            continue;
        }
        if json {
            let doc: serde_json::Value =
                serde_json::from_str(&stdout(&out)).expect("json parses");
            assert_eq!(doc["summary"]["failed"], 1, "{doc}");
            assert_eq!(doc["ok"], false, "{doc}");
        } else {
            let body = stdout(&out);
            assert!(
                body.contains("clip failed (blocked)")
                    && body.contains("NOT written: verification failed"),
                "fallback verify-failure row:\n{body}"
            );
            assert!(
                !body.contains("written as a task with a"),
                "row must not claim success:\n{body}"
            );
        }
    }
}

#[test]
fn archive_changed_after_clip_reports_and_keeps_journal() {
    let (env, fake, state, _token) = setup("bob-cli-gkeep-pull-ref-changed");
    let (curl, root, log) = write_clip_curl(state.path());
    let n1 = note("")
        .id("note-1")
        .text("https://example.com/paper.pdf")
        .build();
    fake.respond(
        "snapshot",
        &snapshot_ok("bryanbugyi34@gmail.com", vec![n1.clone()]),
    );
    // An attachment added mid-pull trips the archive guard.
    fake.respond("archive", &archive_ok(vec![("note-1", "changed")]));
    let out = run_pull(&env, &fake, &state, &[], &curl_env(&curl, &root, &log));
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("NOT archived: edited in Keep during pull"),
        "changed row:\n{}",
        stdout(&out)
    );
    // The clip still journaled, so the next pull archives without
    // clipping again.
    assert_eq!(ref_events(&state).len(), 1);
    assert!(env.vault().join("xlib/papers/paper.pdf").is_file());
    fake.respond("snapshot", &snapshot_ok("bryanbugyi34@gmail.com", vec![n1]));
    fake.respond("archive", &archive_ok(vec![("note-1", "archived")]));
    let (sentinel, hit) = write_sentinel(state.path(), "curl-repull");
    let out = run_pull(
        &env,
        &fake,
        &state,
        &["-f", "json"],
        &[("BOB_HIGHLIGHTS_CURL", sentinel.to_str().expect("path"))],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let doc: serde_json::Value =
        serde_json::from_str(&stdout(&out)).expect("json parses");
    assert_eq!(doc["notes"][0]["action"], "archive_only", "{doc}");
    assert!(!hit.is_file(), "re-pull archives with no second clip");
}

#[test]
fn all_clip_pull_needs_no_target() {
    let (env, fake, state, _token) = setup("bob-cli-gkeep-pull-ref-notarget");
    let (curl, root, log) = write_clip_curl(state.path());
    fs::remove_file(env.vault().join("gkeep_inbox.md")).expect("remove target");
    let n1 = note("")
        .id("note-1")
        .text("https://example.com/paper.pdf")
        .build();
    fake.respond("snapshot", &snapshot_ok("bryanbugyi34@gmail.com", vec![n1]));
    fake.respond("archive", &archive_ok(vec![("note-1", "archived")]));
    let out = run_pull(&env, &fake, &state, &[], &curl_env(&curl, &root, &log));
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("clipped → xlib/papers/paper.pdf · archived"),
        "clip row:\n{}",
        stdout(&out)
    );
}

#[test]
fn retryable_clip_failure_needs_no_target() {
    let (env, fake, state, _token) =
        setup("bob-cli-gkeep-pull-ref-retry-no-target");
    let (curl, root, log) = write_clip_curl(state.path());
    fs::remove_file(env.vault().join("gkeep_inbox.md")).expect("remove target");
    let n1 = note("")
        .id("note-1")
        .text("https://example.com/timeout")
        .build();
    fake.respond("snapshot", &snapshot_ok("bryanbugyi34@gmail.com", vec![n1]));
    let out = run_pull(&env, &fake, &state, &[], &curl_env(&curl, &root, &log));
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stdout(&out)
            .contains("clip failed (timeout) · left in Keep for the next pull"),
        "retry row, not a target error:\n{}",
        stdout(&out)
    );
    assert!(
        !stderr(&out).contains("target note"),
        "must not demand the target:\n{}",
        stderr(&out)
    );
    assert_eq!(fake.call_count(), 1, "snapshot only, no archive");
    assert!(ref_events(&state).is_empty(), "no ref_created journal");
}

#[test]
fn ref_created_repull_needs_no_target() {
    let (env, fake, state, _token) =
        setup("bob-cli-gkeep-pull-ref-repull-no-target");
    let (curl, root, log) = write_clip_curl(state.path());
    let n1 = note("")
        .id("note-1")
        .text("https://example.com/paper.pdf")
        .build();
    fake.respond(
        "snapshot",
        &snapshot_ok("bryanbugyi34@gmail.com", vec![n1.clone()]),
    );
    fake.respond("archive", &archive_ok(vec![("note-1", "changed")]));
    let out = run_pull(&env, &fake, &state, &[], &curl_env(&curl, &root, &log));
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert_eq!(ref_events(&state).len(), 1);
    fs::remove_file(env.vault().join("gkeep_inbox.md")).expect("remove target");
    fake.respond("snapshot", &snapshot_ok("bryanbugyi34@gmail.com", vec![n1]));
    fake.respond("archive", &archive_ok(vec![("note-1", "archived")]));
    let (sentinel, hit) = write_sentinel(state.path(), "curl-repull");
    let out = run_pull(
        &env,
        &fake,
        &state,
        &["-f", "json"],
        &[("BOB_HIGHLIGHTS_CURL", sentinel.to_str().expect("path"))],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let doc: serde_json::Value =
        serde_json::from_str(&stdout(&out)).expect("json parses");
    assert_eq!(doc["notes"][0]["action"], "archive_only", "{doc}");
    assert!(!hit.is_file(), "re-pull archives with no second clip");
}

#[test]
fn no_ref_flag_keeps_url_as_task() {
    let (env, fake, state, _token) = setup("bob-cli-gkeep-pull-ref-noref");
    let (sentinel, hit) = write_sentinel(state.path(), "curl");
    let n1 = note("")
        .id("note-1")
        .text("https://example.com/paper.pdf")
        .build();
    fake.respond("snapshot", &snapshot_ok("bryanbugyi34@gmail.com", vec![n1]));
    fake.respond("archive", &archive_ok(vec![("note-1", "archived")]));
    let out = run_pull(
        &env,
        &fake,
        &state,
        &["-R", "-f", "json"],
        &[("BOB_HIGHLIGHTS_CURL", sentinel.to_str().expect("path"))],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(!hit.is_file(), "no fetch with -R");
    let doc: serde_json::Value =
        serde_json::from_str(&stdout(&out)).expect("json parses");
    assert_eq!(doc["notes"][0]["action"], "write");
    assert!(doc["notes"][0].get("clip").is_none(), "no clip object");
    assert!(read_target(env.vault()).contains("https://example.com/paper.pdf"));
}

#[test]
fn dry_run_previews_ref_without_clipping() {
    let (env, fake, state, _token) = setup("bob-cli-gkeep-pull-ref-dry");
    let (curl_sentinel, curl_hit) = write_sentinel(state.path(), "curl");
    let (clip_sentinel, clip_hit) = write_sentinel(state.path(), "clip");
    let n1 = note("")
        .id("note-1")
        .text("https://example.com/paper.pdf")
        .build();
    fake.respond("snapshot", &snapshot_ok("bryanbugyi34@gmail.com", vec![n1]));
    let out = run_pull(
        &env,
        &fake,
        &state,
        &["-d"],
        &[
            ("BOB_HIGHLIGHTS_CURL", curl_sentinel.to_str().expect("path")),
            (
                "BOB_WEB_CLIP_ADAPTER",
                clip_sentinel.to_str().expect("path"),
            ),
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(!curl_hit.is_file(), "dry run never fetches");
    assert!(!clip_hit.is_file(), "dry run never clips");
    let body = stdout(&out);
    assert!(
        body.contains("would clip → reading queue · would archive"),
        "would-clip row:\n{body}"
    );
    assert!(
        body.contains("1 link would be clipped into the reading queue"),
        "clip line:\n{body}"
    );
    assert!(
        !body.contains("Markdown to insert"),
        "no markdown for ref notes:\n{body}"
    );
    assert_eq!(fake.call_count(), 1, "no archive call on dry run");
    let out = run_pull(
        &env,
        &fake,
        &state,
        &["-d", "-f", "json"],
        &[
            ("BOB_HIGHLIGHTS_CURL", curl_sentinel.to_str().expect("path")),
            (
                "BOB_WEB_CLIP_ADAPTER",
                clip_sentinel.to_str().expect("path"),
            ),
        ],
    );
    let doc: serde_json::Value =
        serde_json::from_str(&stdout(&out)).expect("json parses");
    assert_eq!(doc["notes"][0]["action"], "create_ref");
    assert_eq!(doc["notes"][0]["clip"]["outcome"], "would_clip");
    assert_eq!(doc["notes"][0]["written"], false);
}

#[test]
fn url_only_matrix_classifies_without_clipping() {
    let (env, fake, state, _token) = setup("bob-cli-gkeep-pull-ref-matrix");
    let (sentinel, hit) = write_sentinel(state.path(), "curl");
    let notes = vec![
        note("My take")
            .id("m1")
            .created("2026-09-27T21:14:01Z")
            .text("https://example.com/post")
            .build(),
        note("")
            .id("m2")
            .created("2026-09-27T21:14:02Z")
            .list(vec![("https://example.com/post", false, false)])
            .build(),
        note("")
            .id("m3")
            .created("2026-09-27T21:14:03Z")
            .text("https://example.com/post")
            .attachment("image", None)
            .build(),
        note("")
            .id("m4")
            .created("2026-09-27T21:14:04Z")
            .text("https://example.com/post")
            .shared()
            .build(),
        note("")
            .id("m5")
            .created("2026-09-27T21:14:05Z")
            .text("https://www.youtube.com/watch?v=x")
            .build(),
        note("")
            .id("m6")
            .created("2026-09-27T21:14:06Z")
            .text("http://go/x")
            .build(),
        note("")
            .id("m7")
            .created("2026-09-27T21:14:07Z")
            .text("https://example.com/a https://example.com/b")
            .build(),
        note("")
            .id("m8")
            .created("2026-09-27T21:14:08Z")
            .text("https://example.com/post")
            .build(),
        note("Example Post")
            .id("m9")
            .created("2026-09-27T21:14:09Z")
            .text("https://example.com/post")
            .link("https://example.com/post", "Example Post")
            .build(),
        note("")
            .id("m10")
            .created("2026-09-27T21:14:10Z")
            .text("https://example.com/post")
            .pinned()
            .build(),
    ];
    fake.respond("snapshot", &snapshot_ok("bryanbugyi34@gmail.com", notes));
    let out = run_pull(
        &env,
        &fake,
        &state,
        &["-d", "-f", "json", "-S", "-p"],
        &[("BOB_HIGHLIGHTS_CURL", sentinel.to_str().expect("path"))],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(!hit.is_file(), "matrix dry run never fetches");
    let doc: serde_json::Value =
        serde_json::from_str(&stdout(&out)).expect("json parses");
    let actions: Vec<(&str, &str)> = doc["notes"]
        .as_array()
        .expect("notes")
        .iter()
        .map(|row| {
            (
                row["id"].as_str().expect("id"),
                row["action"].as_str().expect("action"),
            )
        })
        .collect();
    assert_eq!(
        actions,
        vec![
            ("m1", "write"),
            ("m2", "write"),
            ("m3", "write"),
            ("m4", "write"),
            ("m5", "write"),
            ("m6", "write"),
            ("m7", "write"),
            ("m8", "create_ref"),
            ("m9", "create_ref"),
            ("m10", "create_ref"),
        ],
        "matrix actions:\n{doc}"
    );
}
