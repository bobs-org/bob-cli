//! `bob ref jobs` spool, worker, and list tests.
//!
//! Every test isolates `XDG_STATE_HOME`, the vault-sync lock, and the
//! config file through `bob_command()`, and never touches the network:
//! fake curl serves every fetch and `FakeClip` answers every adapter
//! call. The kick stays off except in the kick's own unit tests.

use super::fake_clip::*;
use crate::support::*;
use std::fs;

const ARTICLE_URL: &str = "https://example.com/post";
const FIXED_CREATED_AT: &str = "2026-10-07T14:30:12-04:00";
const TASK_LINE: &str =
    "- [ ] #task https://example.com/post [created::2026-10-07]";

fn state_dir(temp: &TempDir) -> std::path::PathBuf {
    temp.path().join("state")
}

fn jobs_command(
    temp: &TempDir,
    vault: &std::path::Path,
    args: &[&str],
) -> std::process::Command {
    let mut command = bob_command();
    command.env("XDG_STATE_HOME", state_dir(temp));
    command.env("BOB_DIR", vault);
    command.arg("ref").arg("jobs");
    for arg in args {
        command.arg(arg);
    }
    command
}

fn seed_pending(
    temp: &TempDir,
    vault: &std::path::Path,
    id: &str,
    created_at: &str,
) -> std::path::PathBuf {
    seed_pending_with_url(temp, vault, id, created_at, ARTICLE_URL, TASK_LINE)
}

fn seed_pending_with_url(
    temp: &TempDir,
    vault: &std::path::Path,
    id: &str,
    created_at: &str,
    url: &str,
    task_line: &str,
) -> std::path::PathBuf {
    let pending = state_dir(temp).join("bob-cli/ref/jobs/pending");
    fs::create_dir_all(&pending).expect("create pending dir");
    let path = pending.join(format!("{id}.json"));
    let job = serde_json::json!({
        "schema_version": 1,
        "id": id,
        "created_at": created_at,
        "source": "capture",
        "bob_dir": vault,
        "url": url,
        "cleaned_url": url,
        "dedupe_key": url,
        "display": url.trim_start_matches("https://").trim_start_matches("http://"),
        "route_hint": "article",
        "attempts": 0,
        "fallback": {
            "relative_target": "mac_inbox.md",
            "task_line": task_line,
        },
    });
    fs::write(&path, serde_json::to_vec_pretty(&job).expect("encode job"))
        .expect("seed pending job");
    path
}

fn seed_running(
    temp: &TempDir,
    vault: &std::path::Path,
    id: &str,
    attempts: u32,
) {
    let pending = seed_pending(temp, vault, id, FIXED_CREATED_AT);
    let running = state_dir(temp).join("bob-cli/ref/jobs/running");
    fs::create_dir_all(&running).expect("create running dir");
    let mut job: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&pending).expect("read job"))
            .expect("parse job");
    job["attempts"] = serde_json::json!(attempts);
    job["started_at"] = serde_json::json!(FIXED_CREATED_AT);
    fs::write(
        running.join(format!("{id}.json")),
        serde_json::to_vec_pretty(&job).expect("encode job"),
    )
    .expect("seed running job");
    fs::remove_file(&pending).expect("remove pending copy");
}

fn write_article_curl(
    dir: &std::path::Path,
    slow_secs: u64,
) -> std::path::PathBuf {
    let path = dir.join("fake-curl.sh");
    let mut script = String::from(
        "#!/bin/sh\ndest=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"-o\" ]; then dest=\"$arg\"; fi\n  prev=\"$arg\"\n  url=\"$arg\"\ndone\n",
    );
    if slow_secs > 0 {
        script.push_str(&format!("sleep {slow_secs}\n"));
    }
    script.push_str(
        "case \"$url\" in\n  *\"example.com\"*)\n    printf '<html><body>article</body></html>' > \"$dest\"\n    printf '200\\ntext/html; charset=utf-8\\n\\n'\n    ;;\n  *)\n    printf '404\\ntext/html\\n\\n'\n    ;;\nesac\n",
    );
    write_executable(&path, &script);
    path
}

fn done_contents(temp: &TempDir) -> String {
    fs::read_to_string(state_dir(temp).join("bob-cli/ref/jobs/done.jsonl"))
        .unwrap_or_default()
}

#[test]
fn ref_jobs_bare_lists_empty_vault_state() {
    let temp = TempDir::new("bob-cli-ref-jobs-empty");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    for args in [&[][..], &["list"][..]] {
        let output = jobs_command(&temp, &vault, args)
            .output()
            .expect("run bob ref jobs");
        assert_success(&output);
        assert_eq!(
            stdout(&output),
            "bob ref · jobs · nothing pending · nothing in the last 7 days\n",
            "args {args:?}:\n{}",
            format_output(&output)
        );
    }

    // Bare flags rewrite to `list` too.
    let output = jobs_command(&temp, &vault, &["-f", "json"])
        .output()
        .expect("run bob ref jobs -f json");
    assert_success(&output);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("parse json");
    assert_eq!(parsed["schema_version"], 1);
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["jobs"], serde_json::json!([]));
    for key in [
        "pending",
        "clipping",
        "clipped",
        "in_library",
        "already_queued",
        "fell_back",
        "stuck",
    ] {
        assert_eq!(parsed["summary"][key], 0, "summary.{key}");
    }
}

#[test]
fn ref_jobs_run_clips_a_seeded_article() {
    let temp = TempDir::new("bob-cli-ref-jobs-clip");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let fake = FakeClip::new(&temp, "fake");
    let curl = write_article_curl(temp.path(), 0);
    seed_pending(&temp, &vault, "20261007T143012-aaaaaa", FIXED_CREATED_AT);

    let output = jobs_command(&temp, &vault, &["run"])
        .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
        .env("BOB_HIGHLIGHTS_CURL", &curl)
        .output()
        .expect("run bob ref jobs run");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("⟳ clipping example.com/post…")
            && report.contains(
                "✓ clipped example.com/post → xlib/blogs/symphony_spec.pdf"
            )
            && report.contains("ok 1 clipped"),
        "{report}"
    );
    // `post` is a skipped URL segment, so create's stem rule falls
    // back to the adapter title (`symphony_spec`).
    assert!(
        vault.join("xlib/blogs/symphony_spec.pdf").is_file(),
        "intake PDF was clipped"
    );
    assert!(
        !state_dir(&temp)
            .join("bob-cli/ref/jobs/pending/20261007T143012-aaaaaa.json")
            .exists(),
        "pending job file is gone"
    );
    let done = done_contents(&temp);
    assert!(
        done.contains("20261007T143012-aaaaaa")
            && done.contains("\"outcome\":\"created\"")
            && done.contains("xlib/blogs/symphony_spec.pdf"),
        "{done}"
    );
    assert!(fake.called(), "the adapter must clip the article");
}

#[test]
fn ref_jobs_run_falls_back_on_blocked() {
    let temp = TempDir::new("bob-cli-ref-jobs-blocked");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let fake = FakeClip::new(&temp, "fake");
    fake.respond(&failure_response("blocked", "Bot wall", "try later"));
    let curl = write_article_curl(temp.path(), 0);
    seed_pending(&temp, &vault, "20261007T143012-bbbbbb", FIXED_CREATED_AT);

    let output = jobs_command(&temp, &vault, &["run"])
        .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
        .env("BOB_HIGHLIGHTS_CURL", &curl)
        .output()
        .expect("run bob ref jobs run");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report
            .contains("↩ fell back example.com/post → mac_inbox.md (blocked:")
            && report.contains("ok 1 fell back"),
        "{report}"
    );
    let inbox = fs::read_to_string(vault.join("mac_inbox.md"))
        .expect("read fallback target");
    assert!(
        inbox.contains(TASK_LINE) && inbox.contains("⚠️ Clip failed (blocked)"),
        "{inbox}"
    );
    assert!(
        inbox.contains("retry: bob ref create https://example.com/post"),
        "{inbox}"
    );
    let done = done_contents(&temp);
    assert!(
        done.contains("\"outcome\":\"fell_back\"")
            && done.contains("\"kind\":\"blocked\"")
            && done.contains("mac_inbox.md"),
        "{done}"
    );
}

#[test]
fn ref_jobs_fallback_bytes_match_capture_twin_vault() {
    let temp = TempDir::new("bob-cli-ref-jobs-twin");
    let vault_a = temp.path().join("vault-a");
    let vault_b = temp.path().join("vault-b");
    for vault in [&vault_a, &vault_b] {
        fs::create_dir_all(vault).expect("create vault");
    }
    // Routing is on in production; `-R` recovers the routing-off
    // task bytes the fallback must match.
    let captured = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault_a)
        .arg("-R")
        .arg(ARTICLE_URL)
        .env("BOB_NOW", "2026-10-07 12:00:00")
        .output()
        .expect("run bob capture");
    assert_success(&captured);
    let expected =
        fs::read_to_string(vault_a.join("mac_inbox.md")).expect("read twin");
    let task_line = expected.lines().next().expect("task line").to_string();

    let fake = FakeClip::new(&temp, "fake");
    fake.respond(&failure_response("blocked", "Bot wall", "try later"));
    let curl = write_article_curl(temp.path(), 0);
    seed_pending_with_url(
        &temp,
        &vault_b,
        "20261007T143012-cccccc",
        FIXED_CREATED_AT,
        ARTICLE_URL,
        &task_line,
    );
    let output = jobs_command(&temp, &vault_b, &["run"])
        .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
        .env("BOB_HIGHLIGHTS_CURL", &curl)
        .output()
        .expect("run bob ref jobs run");
    assert_success(&output);

    let actual =
        fs::read_to_string(vault_b.join("mac_inbox.md")).expect("read target");
    let expected_lines: Vec<&str> = expected.lines().collect();
    let actual_lines: Vec<&str> = actual.lines().collect();
    assert_eq!(
        actual_lines.len(),
        expected_lines.len() + 1,
        "exactly the capture lines plus the ⚠️ child:\n{actual}"
    );
    assert_eq!(
        &actual_lines[..expected_lines.len()],
        &expected_lines[..],
        "capture bytes first:\n{actual}"
    );
    assert!(
        actual_lines[expected_lines.len()].contains("⚠️ Clip failed (blocked)")
            && actual_lines[expected_lines.len()]
                .contains("retry: bob ref create https://example.com/post"),
        "fallback child last:\n{actual}"
    );
}

#[test]
fn ref_jobs_stale_running_once_is_requeued_and_clipped() {
    let temp = TempDir::new("bob-cli-ref-jobs-recover-once");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let fake = FakeClip::new(&temp, "fake");
    let curl = write_article_curl(temp.path(), 0);
    seed_running(&temp, &vault, "20261007T143012-dddddd", 0);

    let output = jobs_command(&temp, &vault, &["run"])
        .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
        .env("BOB_HIGHLIGHTS_CURL", &curl)
        .output()
        .expect("run bob ref jobs run");
    assert_success(&output);
    assert!(
        stdout(&output).contains("ok 1 clipped"),
        "{}",
        format_output(&output)
    );
    let done = done_contents(&temp);
    assert!(done.contains("\"outcome\":\"created\""), "{done}");
    assert!(
        fs::read_dir(state_dir(&temp).join("bob-cli/ref/jobs/running"))
            .expect("read running")
            .next()
            .is_none(),
        "no stale running files remain"
    );
}

#[test]
fn ref_jobs_stale_running_twice_falls_back_without_clipping() {
    let temp = TempDir::new("bob-cli-ref-jobs-recover-twice");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    seed_running(&temp, &vault, "20261007T143012-eeeeee", 2);

    // The default harness adapter is missing: any clip attempt would
    // fail as `dependency`, so `internal` proves nothing clipped.
    let output = jobs_command(&temp, &vault, &["run"])
        .output()
        .expect("run bob ref jobs run");
    assert_success(&output);
    let done = done_contents(&temp);
    assert!(
        done.contains("\"outcome\":\"fell_back\"")
            && done.contains("\"kind\":\"internal\"")
            && done.contains("the clip worker stopped twice"),
        "{done}"
    );
    let inbox = fs::read_to_string(vault.join("mac_inbox.md"))
        .expect("read fallback target");
    assert!(
        inbox.contains(TASK_LINE)
            && inbox.contains("⚠️ Clip failed (internal)"),
        "{inbox}"
    );
}

#[test]
fn ref_jobs_stuck_fallback_is_retried_without_clipping() {
    let temp = TempDir::new("bob-cli-ref-jobs-stuck");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let pending =
        seed_pending(&temp, &vault, "20261007T143012-ffffff", FIXED_CREATED_AT);
    let stuck = state_dir(&temp).join("bob-cli/ref/jobs/stuck");
    fs::create_dir_all(&stuck).expect("create stuck dir");
    let mut job: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&pending).expect("read job"))
            .expect("parse job");
    job["error"] = serde_json::json!({
        "kind": "timeout",
        "message": "timed out",
        "retryable": true,
    });
    job["fallback_error"] = serde_json::json!("disk was busy");
    fs::write(
        stuck.join("20261007T143012-ffffff.json"),
        serde_json::to_vec_pretty(&job).expect("encode job"),
    )
    .expect("seed stuck job");
    fs::remove_file(&pending).expect("remove pending copy");

    // No clip env at all: the retry must not touch the network.
    let output = jobs_command(&temp, &vault, &["run"])
        .output()
        .expect("run bob ref jobs run");
    assert_success(&output);
    assert!(
        stdout(&output).contains("ok 1 fell back"),
        "{}",
        format_output(&output)
    );
    let inbox = fs::read_to_string(vault.join("mac_inbox.md"))
        .expect("read fallback target");
    assert!(
        inbox.contains(TASK_LINE) && inbox.contains("⚠️ Clip failed (timeout)"),
        "{inbox}"
    );
    assert!(
        fs::read_dir(&stuck).expect("read stuck").next().is_none(),
        "stuck job is gone"
    );
}

#[test]
fn ref_jobs_second_worker_exits_zero() {
    use fs2::FileExt;
    let temp = TempDir::new("bob-cli-ref-jobs-single-flight");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let root = state_dir(&temp).join("bob-cli/ref/jobs");
    fs::create_dir_all(&root).expect("create spool");
    let lock_path = root.join("worker.lock");
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .expect("open worker lock");
    lock.try_lock_exclusive().expect("hold worker lock");

    let output = jobs_command(&temp, &vault, &["run"])
        .output()
        .expect("run second worker");
    assert_success(&output);
    assert!(
        stdout(&output).contains("another clip worker is running"),
        "{}",
        format_output(&output)
    );
}

#[test]
fn ref_jobs_slow_clip_still_drains_a_late_job() {
    let temp = TempDir::new("bob-cli-ref-jobs-wakeup");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let fake = FakeClip::new(&temp, "fake");
    let curl = write_article_curl(temp.path(), 3);
    seed_pending(&temp, &vault, "20261007T143012-111111", FIXED_CREATED_AT);

    let mut child = jobs_command(&temp, &vault, &["run"])
        .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
        .env("BOB_HIGHLIGHTS_CURL", &curl)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn worker");
    // Wait until the first job is claimed (the slow fetch holds the
    // worker), then queue a second job with no further kick.
    let running = state_dir(&temp).join("bob-cli/ref/jobs/running");
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let claimed = fs::read_dir(&running)
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(false);
        if claimed {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "worker never claimed the first job"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    seed_pending_with_url(
        &temp,
        &vault,
        "20261007T143012-222222",
        FIXED_CREATED_AT,
        "https://example.com/late",
        "- [ ] #task https://example.com/late [created::2026-10-07]",
    );
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(90);
    let status = loop {
        match child.try_wait().expect("poll worker") {
            Some(status) => break status,
            None => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "worker never exited"
                );
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    };
    assert!(status.success(), "worker exit: {status}");
    let done = done_contents(&temp);
    assert!(
        done.contains("20261007T143012-111111")
            && done.contains("20261007T143012-222222"),
        "one pass drained both jobs:\n{done}"
    );
}

#[test]
fn ref_jobs_list_windows_and_json() {
    use chrono::{Local, SecondsFormat};
    let temp = TempDir::new("bob-cli-ref-jobs-window");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let now = Local::now().to_rfc3339_opts(SecondsFormat::Secs, false);
    let done_path = state_dir(&temp).join("bob-cli/ref/jobs/done.jsonl");
    fs::create_dir_all(done_path.parent().expect("parent"))
        .expect("create spool");
    fs::write(
        &done_path,
        format!(
            "{{\"schema_version\":1,\"id\":\"old-1\",\"url\":\"https://example.com/old\",\"cleaned_url\":\"https://example.com/old\",\"display\":\"example.com/old\",\"outcome\":\"created\",\"pdf\":\"xlib/blogs/old.pdf\",\"created_at\":\"2020-01-01T00:00:00-05:00\",\"finished_at\":\"2020-01-02T00:00:00-05:00\"}}\n\
             {{\"schema_version\":1,\"id\":\"new-1\",\"url\":\"https://example.com/new\",\"cleaned_url\":\"https://example.com/new\",\"display\":\"example.com/new\",\"outcome\":\"created\",\"pdf\":\"xlib/blogs/new.pdf\",\"created_at\":\"{now}\",\"finished_at\":\"{now}\"}}\n",
        ),
    )
    .expect("seed done log");
    seed_pending(&temp, &vault, "20261007T143012-333333", FIXED_CREATED_AT);

    let output = jobs_command(&temp, &vault, &[])
        .output()
        .expect("run bob ref jobs");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("1 pending · 1 clipped · last 7 days")
            && report.contains("example.com/post")
            && report.contains("example.com/new")
            && !report.contains("example.com/old"),
        "{report}"
    );

    let output = jobs_command(&temp, &vault, &["--all"])
        .output()
        .expect("run bob ref jobs --all");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("· all") && report.contains("example.com/old"),
        "{report}"
    );

    let output = jobs_command(&temp, &vault, &["-f", "json"])
        .output()
        .expect("run bob ref jobs -f json");
    assert_success(&output);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("parse json");
    assert_eq!(parsed["summary"]["pending"], 1);
    assert_eq!(parsed["summary"]["clipped"], 1);
    assert_eq!(parsed["jobs"].as_array().expect("jobs").len(), 2);
}

fn commit_vault(vault: &std::path::Path) {
    // Git tracks no empty dirs: anchor the commit with a file the
    // doctor ignores.
    write_file(&vault.join("lib/.keep"), "");
    git_in(vault, ["init", "-q"]);
    configure_test_git_identity(vault);
    git_in(vault, ["add", "."]);
    git_in(vault, ["commit", "-q", "-m", "initial vault"]);
}

#[test]
fn ref_jobs_doctor_row_tracks_pending_and_stuck() {
    let temp = TempDir::new("bob-cli-ref-jobs-doctor");
    let vault = temp.path().join("vault");
    for dir in ["lib", "ref", "xlib"] {
        fs::create_dir_all(vault.join(dir)).expect("create vault dir");
    }
    commit_vault(&vault);
    let doctor = || {
        bob_command()
            .arg("ref")
            .arg("doctor")
            .arg("--no-hooks")
            .arg("-b")
            .arg(&vault)
            .env("XDG_STATE_HOME", state_dir(&temp))
            .output()
            .expect("run bob ref doctor")
    };

    let output = doctor();
    assert_success(&output);
    assert!(
        stdout(&output).contains("ref jobs: ok (nothing pending)"),
        "{}",
        format_output(&output)
    );

    // A two-hour-old pending job warns with the run hint.
    use chrono::{Duration, Local, SecondsFormat};
    let old = (Local::now() - Duration::hours(2))
        .to_rfc3339_opts(SecondsFormat::Secs, false);
    seed_pending(&temp, &vault, "20261007T143012-444444", &old);
    let output = doctor();
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains(
            "ref jobs: warn (1 pending · oldest 2 h · hint: bob ref jobs run)"
        ) && report.contains("warnings:"),
        "{report}"
    );
}
