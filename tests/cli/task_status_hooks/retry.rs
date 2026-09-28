//! Lock, retry budget, cron redirection.

use crate::support::*;
use fs2::FileExt;
use std::fs;
use std::io;
use std::io::BufRead;
use std::path::Path;
use std::process::Command;
use std::process::Stdio;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use std::time::Instant;

#[test]
fn task_status_hooks_dry_run_creates_no_lock_or_recovery() {
    let temp = TempDir::new("bob-cli-task-status-hooks-dry-lock");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260710.md");
    let tasks = vault.join("tasks.md");
    let lock = temp.path().join("bob_sync.lock");
    let state = temp.path().join("state");
    write_file(&daily, "## Pomodoros\n\n- [ ] Current (0900-0930)\n");
    write_file(&tasks, "- [*] #task Stale next ^stale\n");

    let output = bob_command()
        .arg("task-status-hooks")
        .arg("--dry-run")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .env("BOB_VAULT_SYNC_LOCK_FILE", &lock)
        .env("XDG_STATE_HOME", &state)
        .output()
        .expect("dry-run guarded write");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("dry-run JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["dry_run"], true);
    assert!(!json["cleared"].as_array().unwrap().is_empty());
    assert!(
        !lock.exists(),
        "dry-run must not create the maintenance lock"
    );
    assert!(
        !state.join("bob-cli/task-status-hooks").exists(),
        "dry-run must not write recovery records"
    );
    assert_eq!(
        fs::read_to_string(&tasks).unwrap(),
        "- [*] #task Stale next ^stale\n"
    );
}

#[test]
fn task_status_hooks_live_noop_may_lock_but_creates_no_recovery() {
    let temp = TempDir::new("bob-cli-task-status-hooks-live-noop");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260710.md");
    let tasks = vault.join("tasks.md");
    let lock = temp.path().join("bob_sync.lock");
    let state = temp.path().join("state");
    write_file(&daily, "## Pomodoros\n");
    write_file(&tasks, "- [ ] #task Ready intake ^ready\n");

    let output = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .env("BOB_VAULT_SYNC_LOCK_FILE", &lock)
        .env("XDG_STATE_HOME", &state)
        .output()
        .expect("live no-op guarded write");
    assert_success(&output);
    assert!(
        stdout(&output).contains("already in sync"),
        "expected no-op report:\n{}",
        format_output(&output)
    );
    assert!(lock.exists(), "live no-op may create the maintenance lock");
    assert!(
        !state.join("bob-cli/task-status-hooks").exists(),
        "live no-op must not write recovery records"
    );
}

#[test]
fn task_status_hooks_defers_when_maintenance_lock_is_held() {
    let temp = TempDir::new("bob-cli-task-status-hooks-lock-held");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260710.md");
    let tasks = vault.join("tasks.md");
    let lock_path = temp.path().join("bob_sync.lock");
    write_file(&daily, "## Pomodoros\n");
    write_file(&tasks, "- [*] #task Stale next ^stale\n");

    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .expect("open lock");
    lock.try_lock_exclusive().expect("hold lock");

    let output = bob_command()
        .arg("task-status-hooks")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        // Fail fast instead of waiting out the new production default
        // (120s) retry budget for lock contention.
        .arg("-r")
        .arg("0")
        .env("BOB_DAY_FILE", &daily)
        .env("BOB_VAULT_SYNC_LOCK_FILE", &lock_path)
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .output()
        .expect("run contended task-status-hooks");
    assert_eq!(output.status.code(), Some(1));
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("lock JSON");
    assert_eq!(json["ok"], false);
    assert_eq!(json["reason"], "lock_contention");
    assert!(json["error"]
        .as_str()
        .unwrap()
        .contains("another Bob vault maintenance run"));
    assert_eq!(json["applied_files"].as_array().unwrap().len(), 0);
    assert_eq!(
        fs::read_to_string(&tasks).unwrap(),
        "- [*] #task Stale next ^stale\n"
    );
    assert!(
        stderr(&output).is_empty(),
        "a single fail-fast attempt must not print retry diagnostics:\n{}",
        format_output(&output)
    );
}

#[test]
fn task_status_hooks_human_retry_progress_goes_to_stdout() {
    let temp = TempDir::new("bob-cli-task-status-hooks-retry-success");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260710.md");
    let tasks = vault.join("tasks.md");
    let lock_path = temp.path().join("bob_sync.lock");
    write_file(&daily, "## Pomodoros\n");
    write_file(&tasks, "- [*] #task Stale next ^stale\n");

    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .expect("open lock");
    lock.try_lock_exclusive().expect("hold lock");

    let mut child = bob_command()
        .arg("task-status-hooks")
        .arg("--bob-dir")
        .arg(&vault)
        .arg("-r")
        .arg("30")
        .env("BOB_DAY_FILE", &daily)
        .env("BOB_VAULT_SYNC_LOCK_FILE", &lock_path)
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn contended task-status-hooks");

    let stdout_pipe = child.stdout.take().expect("stdout pipe");
    let (tx, rx) = mpsc::channel();
    let reader = thread::spawn(move || {
        for line in io::BufReader::new(stdout_pipe).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    let mut seen = Vec::new();
    let mut saw_retry_decision = false;
    while let Ok(line) = rx.recv_timeout(Duration::from_secs(20)) {
        let is_decision =
            line.contains("retry run=") && line.contains("attempt=1");
        seen.push(line);
        if is_decision {
            saw_retry_decision = true;
            break;
        }
    }
    if !saw_retry_decision {
        let _ = child.kill();
        let _ = child.wait();
        panic!(
            "expected a retry diagnostic on stdout before the lock was released; saw:\n{}",
            seen.join("\n")
        );
    }

    lock.unlock().expect("release lock");
    // Confirm the lock is free during the child's backoff window: a fresh
    // handle can also take it immediately, proving backoff never holds the
    // shared maintenance lock.
    let verifier = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock_path)
        .expect("open lock for verification");
    verifier
        .try_lock_exclusive()
        .expect("the lock must be free while the child backs off");
    verifier.unlock().expect("release verification hold");
    drop(verifier);
    drop(lock);

    // Drain the remaining retry diagnostics and final human report from the
    // child's stdout pipe while we wait for it to exit.
    for line in rx.iter() {
        seen.push(line);
    }
    let _ = reader.join();

    let output = child
        .wait_with_output()
        .expect("wait for retrying task-status-hooks");
    assert!(
        output.status.success(),
        "retry must eventually succeed once the lock is free:\n{}\nstderr:\n{}",
        stdout(&output),
        seen.join("\n")
    );
    assert!(
        stderr(&output).is_empty(),
        "human retry success without warnings must keep stderr empty:\n{}",
        stderr(&output)
    );
    assert!(
        seen.iter().any(|line| line.contains("succeeded")),
        "expected a retry success summary; saw:\n{}",
        seen.join("\n")
    );
    let human_output = seen.join("\n");
    assert!(
        human_output.contains("retry run=")
            && human_output.contains("attempt=1")
            && human_output.contains("Summary:")
            && human_output.contains("bob task-status-hooks"),
        "expected retry progress and final human report on stdout:\n{human_output}"
    );
    assert_eq!(
        fs::read_to_string(&tasks).unwrap(),
        "- [ ] #task Stale next ^stale\n",
        "the eventually successful retry must still apply the guarded write"
    );
}

#[test]
fn task_status_hooks_json_retry_progress_stays_off_stdout() {
    let temp = TempDir::new("bob-cli-task-status-hooks-json-retry-success");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260710.md");
    let tasks = vault.join("tasks.md");
    let lock_path = temp.path().join("bob_sync.lock");
    write_file(&daily, "## Pomodoros\n");
    write_file(&tasks, "- [*] #task Stale next ^stale\n");

    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .expect("open lock");
    lock.try_lock_exclusive().expect("hold lock");

    let mut child = bob_command()
        .arg("task-status-hooks")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .arg("-r")
        .arg("30")
        .env("BOB_DAY_FILE", &daily)
        .env("BOB_VAULT_SYNC_LOCK_FILE", &lock_path)
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn contended JSON task-status-hooks");

    let stderr_pipe = child.stderr.take().expect("stderr pipe");
    let (tx, rx) = mpsc::channel();
    let reader = thread::spawn(move || {
        for line in io::BufReader::new(stderr_pipe).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    let mut seen = Vec::new();
    let mut saw_retry_decision = false;
    while let Ok(line) = rx.recv_timeout(Duration::from_secs(20)) {
        let is_decision =
            line.contains("retry run=") && line.contains("attempt=1");
        seen.push(line);
        if is_decision {
            saw_retry_decision = true;
            break;
        }
    }
    if !saw_retry_decision {
        let _ = child.kill();
        let _ = child.wait();
        panic!(
            "expected a retry diagnostic on stderr before the lock was released; saw:\n{}",
            seen.join("\n")
        );
    }

    lock.unlock().expect("release lock");
    drop(lock);

    for line in rx.iter() {
        seen.push(line);
    }
    let _ = reader.join();

    let output = child
        .wait_with_output()
        .expect("wait for retrying JSON task-status-hooks");
    assert!(
        output.status.success(),
        "retry must eventually succeed once the lock is free:\n{}",
        format_output(&output)
    );
    let out = stdout(&output);
    assert!(
        !out.contains("retry run="),
        "JSON stdout must not be interleaved with retry diagnostics:\n{out}"
    );
    let json: serde_json::Value =
        serde_json::from_str(out.trim()).expect("retry JSON");
    assert_eq!(json["ok"], true);
    assert!(
        seen.iter().any(|line| line.contains("succeeded")),
        "expected a retry success summary on stderr; saw:\n{}",
        seen.join("\n")
    );
    assert_eq!(
        fs::read_to_string(&tasks).unwrap(),
        "- [ ] #task Stale next ^stale\n",
        "the eventually successful retry must still apply the guarded write"
    );
}

#[test]
fn task_status_hooks_exhausts_retry_budget_and_still_fails() {
    let temp = TempDir::new("bob-cli-task-status-hooks-retry-exhausted");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260710.md");
    let tasks = vault.join("tasks.md");
    let lock_path = temp.path().join("bob_sync.lock");
    write_file(&daily, "## Pomodoros\n");
    write_file(&tasks, "- [*] #task Stale next ^stale\n");

    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .expect("open lock");
    lock.try_lock_exclusive().expect("hold lock");

    let output = bob_command()
        .arg("task-status-hooks")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .arg("-r")
        .arg("1")
        .env("BOB_DAY_FILE", &daily)
        .env("BOB_VAULT_SYNC_LOCK_FILE", &lock_path)
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .output()
        .expect("run exhausted-retry task-status-hooks");
    drop(lock);

    assert_eq!(output.status.code(), Some(1));
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("exhausted JSON");
    assert_eq!(json["ok"], false);
    assert_eq!(json["reason"], "lock_contention");
    let diagnostics = stderr(&output);
    assert!(
        diagnostics.contains("retry run="),
        "expected retry diagnostics:\n{diagnostics}"
    );
    assert!(
        diagnostics.contains("exhausted retry budget"),
        "expected an exhaustion summary, not a false success claim:\n{diagnostics}"
    );
    assert_eq!(
        fs::read_to_string(&tasks).unwrap(),
        "- [*] #task Stale next ^stale\n",
        "an exhausted retry budget must not write notes"
    );
}

#[test]
fn task_status_hooks_dry_run_ignores_retry_timeout() {
    let temp = TempDir::new("bob-cli-task-status-hooks-dry-run-retry");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260710.md");
    let tasks = vault.join("tasks.md");
    let lock = temp.path().join("bob_sync.lock");
    let state = temp.path().join("state");
    write_file(&daily, "## Pomodoros\n\n- [ ] Current (0900-0930)\n");
    write_file(&tasks, "- [*] #task Stale next ^stale\n");

    let start = Instant::now();
    let output = bob_command()
        .arg("task-status-hooks")
        .arg("--dry-run")
        .arg("--retry-timeout")
        .arg("300")
        .arg("--format")
        .arg("json")
        .arg("--bob-dir")
        .arg(&vault)
        .env("BOB_DAY_FILE", &daily)
        .env("BOB_VAULT_SYNC_LOCK_FILE", &lock)
        .env("XDG_STATE_HOME", &state)
        .output()
        .expect("dry-run with a retry timeout supplied");
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "dry-run must never enter the retry controller's sleep behavior"
    );
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("dry-run JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["dry_run"], true);
    assert!(
        !lock.exists(),
        "dry-run must not create the maintenance lock even with --retry-timeout set"
    );
    assert!(
        stderr(&output).is_empty(),
        "dry-run must not print retry diagnostics"
    );
}

#[test]
fn task_status_hooks_rejects_invalid_retry_timeout() {
    for value in ["-1", "abc", "99999999999999999999"] {
        let output = bob_command()
            .arg("task-status-hooks")
            // `=` keeps clap from treating a leading `-` as a new flag.
            .arg(format!("--retry-timeout={value}"))
            .arg("--dry-run")
            .output()
            .expect("run task-status-hooks with an invalid --retry-timeout");
        assert!(
            !output.status.success(),
            "--retry-timeout {value} must be rejected"
        );
        assert!(
            stderr(&output).contains("retry-timeout"),
            "expected a clear diagnostic naming --retry-timeout for {value}:\n{}",
            format_output(&output)
        );
    }
}

#[test]
fn task_status_hooks_cron_redirection_captures_retry_and_final_result() {
    let temp = TempDir::new("bob-cli-task-status-hooks-cron-retry-log");
    let vault = temp.path().join("vault");
    let daily = vault.join("2026/20260710.md");
    let tasks = vault.join("tasks.md");
    let lock_path = temp.path().join("bob_sync.lock");
    let state = temp.path().join("state");
    let log_path = temp.path().join("bob_task_status_hooks.log");
    write_file(&daily, "## Pomodoros\n");
    write_file(&tasks, "- [*] #task Stale next ^stale\n");

    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .expect("open lock");
    lock.try_lock_exclusive().expect("hold lock");

    let mut child = Command::new("sh")
        .arg("-c")
        .arg(r#"exec "$0" task-status-hooks --bob-dir "$1" -r 30 >> "$2""#)
        .arg(BOB_BIN)
        .arg(&vault)
        .arg(&log_path)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", temp.path())
        .env("BOB_DAY_FILE", &daily)
        .env("BOB_VAULT_SYNC_LOCK_FILE", &lock_path)
        .env("XDG_STATE_HOME", &state)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn cron-style task-status-hooks");

    let log_during_contention = poll_log_until_contains(
        &log_path,
        "retry run=",
        Duration::from_secs(20),
    );
    if !log_during_contention.contains("retry run=") {
        let _ = child.kill();
        let _ = child.wait();
        panic!(
            "expected a retry diagnostic in the cron log before releasing the lock:\n{log_during_contention}"
        );
    }

    lock.unlock().expect("release lock");
    drop(lock);

    let output = child
        .wait_with_output()
        .expect("wait for cron-style task-status-hooks");
    assert_eq!(output.status.code(), Some(0));
    assert!(
        stdout(&output).is_empty(),
        "parent stdout must stay empty under stdout log redirection:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).is_empty(),
        "parent stderr must stay empty when a successful retry has no diagnostics:\n{}",
        format_output(&output)
    );

    let log = fs::read_to_string(&log_path).expect("read cron log");
    assert!(
        log.contains("retry run="),
        "expected retry diagnostics in the log:\n{log}"
    );
    assert!(
        log.contains("succeeded"),
        "expected a retry success summary in the log:\n{log}"
    );
    assert!(
        log.contains("Summary:"),
        "expected the final human result in the log:\n{log}"
    );
    assert!(
        log.contains("bob task-status-hooks"),
        "expected the human command report in the log:\n{log}"
    );
    assert_eq!(
        fs::read_to_string(&tasks).unwrap(),
        "- [ ] #task Stale next ^stale\n",
        "the eventually successful retry must still apply the guarded write"
    );
}

#[test]
fn task_status_hooks_cron_redirection_captures_terminal_failure_and_exit_status(
) {
    let temp = TempDir::new("bob-cli-task-status-hooks-cron-terminal-log");
    let vault = temp.path().join("vault");
    // Never created, so the run fails immediately with a non-retryable
    // "daily note does not exist" error and no lock contention at all.
    let daily = vault.join("2026/20260710.md");
    let lock_path = temp.path().join("bob_sync.lock");
    let state = temp.path().join("state");
    let log_path = temp.path().join("bob_task_status_hooks.log");
    fs::create_dir_all(&vault).expect("create empty vault");

    let output = Command::new("sh")
        .arg("-c")
        .arg(r#"exec "$0" task-status-hooks --bob-dir "$1" -r 30 >> "$2""#)
        .arg(BOB_BIN)
        .arg(&vault)
        .arg(&log_path)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", temp.path())
        .env("BOB_DAY_FILE", &daily)
        .env("BOB_VAULT_SYNC_LOCK_FILE", &lock_path)
        .env("XDG_STATE_HOME", &state)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run terminal-failure task-status-hooks under sh redirection");

    assert_eq!(output.status.code(), Some(1));
    assert!(
        stdout(&output).is_empty(),
        "parent stdout must stay empty under redirection:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("daily note does not exist"),
        "stdout-only redirection must leave human terminal errors on stderr:\n{}",
        format_output(&output)
    );

    assert!(
        stderr(&output).contains("bob task-status-hooks"),
        "expected the command name in the terminal diagnostic:\n{}",
        format_output(&output)
    );
    let log = fs::read_to_string(&log_path).unwrap_or_default();
    assert!(
        !log.contains("daily note does not exist"),
        "stdout-only cron logging must not swallow human terminal errors:\n{log}"
    );
    assert!(
        !log.contains("retry run="),
        "a non-retryable failure must not print retry diagnostics:\n{log}"
    );
}

/// Poll `path` until its contents contain `needle` or `deadline` elapses,
/// returning whatever was last read. Used instead of a fixed sleep to wait
/// for a cron-redirected log to receive a retry diagnostic.
fn poll_log_until_contains(
    path: &Path,
    needle: &str,
    deadline: Duration,
) -> String {
    let start = Instant::now();
    loop {
        if let Ok(contents) = fs::read_to_string(path)
            && contents.contains(needle)
        {
            return contents;
        }
        if start.elapsed() >= deadline {
            return fs::read_to_string(path).unwrap_or_default();
        }
        thread::sleep(Duration::from_millis(50));
    }
}
