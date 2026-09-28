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
    let text = format!(
        "gkeep:\n  email: bryanbugyi34@gmail.com\n  token_command: \"{}\"\n",
        token_script.to_string_lossy().replace('"', "\\\"")
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

    // One commit touching only the target.
    assert_eq!(git_log_names(env.vault()), vec!["gkeep_inbox.md"]);

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
    for line in dry_out.lines().filter_map(|line| line.split_once("│ ")) {
        let preview = line.1.trim();
        if preview.starts_with("- [ ]") {
            assert!(after.contains(preview), "missing {preview} in:\n{after}");
        }
    }
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
    let _ = before;
}
