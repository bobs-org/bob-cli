//! Command execution tests: preview, guarded apply, lock ownership, bounded
//! retries, partial apply, and exact replay.
//!
//! Everything runs in-process against temporary vaults, temporary state and
//! lock locations, and injected engine hooks — never sleeps or live state.
use super::*;

use std::{cell::Cell, rc::Rc};

use serde_json::Value;

use super::super::command::{
    execute, execute_standalone, run_collection_with, Clock, Hooks,
};
use super::super::report::{render_human, render_json, Outcome, Report};

fn fake_clock() -> Clock {
    let now = Rc::new(Cell::new(Duration::ZERO));
    let read = now.clone();
    Clock {
        elapsed: Box::new(move || read.get()),
        sleep: Box::new(move |pause| now.set(now.get() + pause)),
    }
}

/// Hooks over the production planner with a test session per attempt.
/// `customize` sees the session and the zero-based attempt number.
fn hooks(
    state: &Path,
    mut customize: impl FnMut(&mut ApplySession, usize) + 'static,
) -> Hooks {
    let state = state.to_path_buf();
    let attempts = Cell::new(0usize);
    Hooks {
        plan: Box::new(plan_archive),
        session: Box::new(move |vault| {
            let attempt = attempts.get();
            attempts.set(attempt + 1);
            let mut session = test_session(&state, vault);
            customize(&mut session, attempt);
            session
        }),
        child_env: Box::new(Vec::new),
        clock: fake_clock(),
        budget: Duration::from_secs(30),
        lock_wait: Duration::from_millis(300),
    }
}

fn plain_hooks(state: &Path) -> Hooks {
    hooks(state, |_, _| {})
}

/// The lock is free again. Polls briefly: a child forked by a parallel test
/// can transiently inherit a just-closed descriptor, delaying the release.
fn lock_is_released() -> bool {
    ob::acquire_lock_waiting(Duration::from_secs(5), || {}).is_ok()
}

fn lock_env(dir: &Path) -> bob_env::TestEnvGuard {
    bob_env::TestEnvGuard::set(&[(
        "BOB_VAULT_SYNC_LOCK_FILE",
        Some(dir.join("bob_sync.lock").as_os_str()),
    )])
}

fn work_note() -> String {
    area_note(
        "\
- [x] done one #task ^one
- [x] done two #task
- [ ] active #task
",
    )
}

fn fixture(prefix: &str) -> TempDir {
    let vault = TempDir::new(prefix);
    write_file(&vault.path().join("work.md"), &work_note());
    vault
}

fn archive_note(body: &str) -> String {
    format!("---\nparent: \"[[work]]\"\ntype: \"[[done]]\"\n---\n\n{body}")
}

fn report_for(vault: &TempDir, dry_run: bool) -> Report {
    Report::new(vault.path().to_path_buf(), 1, dry_run)
}

fn json(report: &Report) -> Value {
    serde_json::from_str(&render_json(report)).expect("report is JSON")
}

fn rel(paths: &[&str]) -> Vec<PathBuf> {
    paths.iter().map(PathBuf::from).collect()
}

#[test]
fn dry_run_plans_without_lock_session_state_or_writes() {
    let vault = fixture("bob-cli-archive-exec-dry");
    let scratch = TempDir::new("bob-cli-archive-exec-dry-scratch");
    let state = scratch.path().join("never-created");
    let _env = lock_env(scratch.path());
    // A competing run holds the maintenance lock for the whole preview.
    let _held = ob::try_acquire_lock().expect("hold the lock");
    let mut hooks = hooks(&state, |_, _| {
        panic!("a preview must not open a guarded apply session")
    });
    let before = fs::read(vault.path().join("work.md")).expect("read");

    let mut report = report_for(&vault, true);
    execute_standalone(&mut report, &mut hooks);

    assert_eq!(report.exit.outcome, Outcome::Preview);
    assert_eq!(report.exit_code(), 0);
    assert_eq!(fs::read(vault.path().join("work.md")).unwrap(), before);
    assert!(!vault.path().join("done").exists(), "no archive directory");
    assert!(!state.exists(), "no recovery state");
    let value = json(&report);
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["dry_run"], true);
    assert_eq!(value["exit"]["outcome"], "preview");
    assert_eq!(value["exit"]["applied_paths"], serde_json::json!([]));
    assert_eq!(value["git"]["outcome"], "not_run");
    assert_eq!(value["git"]["skip_reason"], "dry_run");
    let moves = value["moves"].as_array().expect("moves");
    assert_eq!(moves.len(), 1);
    assert_eq!(moves[0]["source"], "work.md");
    assert_eq!(moves[0]["archive"], "done/work_done.md");
    assert_eq!(moves[0]["task_count"], 2);
    assert_eq!(moves[0]["creates_archive"], true);
    assert_eq!(
        moves[0]["roots"],
        serde_json::json!([
            {"line": 5, "block_id": "one"},
            {"line": 6, "block_id": null}
        ])
    );
    assert_eq!(moves[0]["block_ids"], serde_json::json!(["one"]));
    assert_eq!(moves[0]["id_renames"], serde_json::json!([]));
    assert_eq!(moves[0]["replays"], serde_json::json!([]));
}

#[test]
fn apply_reports_actual_paths_and_holds_the_lock_throughout() {
    let vault = fixture("bob-cli-archive-exec-apply");
    let scratch = TempDir::new("bob-cli-archive-exec-apply-scratch");
    let state = scratch.path().join("state");
    let _env = lock_env(scratch.path());
    let mut hooks = plain_hooks(&state);
    let planned_under_lock = Rc::new(Cell::new(false));
    let observed = planned_under_lock.clone();
    hooks.plan = Box::new(move |vault, threshold| {
        observed.set(matches!(
            ob::try_acquire_lock(),
            Err(ob::LockAcquireError::Contended)
        ));
        plan_archive(vault, threshold)
    });

    let mut report = report_for(&vault, false);
    execute_standalone(&mut report, &mut hooks);

    assert!(planned_under_lock.get(), "planning must run under the lock");
    assert!(lock_is_released(), "the lock is released");
    assert_eq!(report.exit.outcome, Outcome::Applied, "{:?}", report.exit);
    assert_eq!(report.exit_code(), 0);
    assert_eq!(
        report.exit.applied_paths,
        rel(&["done/work_done.md", "work.md"]),
        "archive installs before the source removal"
    );
    assert!(report.exit.deferred_paths.is_empty());
    let recovery = report.exit.recovery_directory.clone().expect("recovery");
    assert!(recovery.starts_with(&state), "{}", recovery.display());
    assert!(recovery.exists());
    // A temporary directory is not a worktree: Git is skipped with a warning.
    assert_eq!(report.git.skip_reason, Some("not_worktree"));
    assert!(report.git.warning.is_some());
    let archive =
        fs::read_to_string(vault.path().join("done/work_done.md")).unwrap();
    assert!(archive.contains("- [x] done one #task ^one"), "{archive}");
    assert!(archive.contains("- [x] done two #task"), "{archive}");
    let source = fs::read_to_string(vault.path().join("work.md")).unwrap();
    assert!(!source.contains("done one"), "{source}");
    assert!(source.contains("- [ ] active #task"), "{source}");

    let value = json(&report);
    assert_eq!(value["exit"]["outcome"], "applied");
    assert_eq!(
        value["exit"]["applied_paths"],
        serde_json::json!(["done/work_done.md", "work.md"])
    );
    assert_eq!(value["git"]["outcome"], "skipped");
    assert_eq!(value["git"]["skip_reason"], "not_worktree");
    let text = render_human(&report);
    for label in ["scan:", "holds:", "skips:", "grandfathered:", "moves:"] {
        assert!(text.contains(label), "missing {label}:\n{text}");
    }
    assert!(text.contains("git:\n  warning: vault is not a git worktree"));
    assert!(text.contains("summary:\n  moved task blocks: 2"), "{text}");
}

#[test]
fn empty_plan_is_a_noop_that_skips_git() {
    let vault = TempDir::new("bob-cli-archive-exec-noop");
    write_file(
        &vault.path().join("work.md"),
        &area_note("- [ ] active #task\n"),
    );
    let scratch = TempDir::new("bob-cli-archive-exec-noop-scratch");
    let _env = lock_env(scratch.path());
    let mut hooks = hooks(&scratch.path().join("state"), |_, _| {
        panic!("an empty plan needs no guarded session")
    });

    let mut report = report_for(&vault, false);
    execute_standalone(&mut report, &mut hooks);

    assert_eq!(report.exit.outcome, Outcome::Noop);
    assert_eq!(report.exit_code(), 0);
    assert!(!vault.path().join("done").exists());
    let value = json(&report);
    assert_eq!(value["exit"]["outcome"], "noop");
    assert_eq!(value["git"]["outcome"], "skipped");
    assert_eq!(value["git"]["skip_reason"], "no_changes");
    assert_eq!(value["moves"], serde_json::json!([]));
    assert!(render_human(&report).contains(
        "git:\n  skipped: no vault changes\nsummary:\n  no task blocks met the threshold; no vault changes made.\n"
    ));
}

#[test]
fn stale_plan_is_rebuilt_without_losing_the_concurrent_edit() {
    let vault = fixture("bob-cli-archive-exec-stale");
    let scratch = TempDir::new("bob-cli-archive-exec-stale-scratch");
    let _env = lock_env(scratch.path());
    let source = vault.path().join("work.md");
    let attempts = Rc::new(Cell::new(0usize));
    let seen = attempts.clone();
    let edited = source.clone();
    let mut hooks = hooks(&scratch.path().join("state"), move |session, n| {
        seen.set(n + 1);
        if n == 0 {
            let edited = edited.clone();
            session.before_preflight = Some(Box::new(move || {
                let mut text = fs::read_to_string(&edited).unwrap();
                text.push_str("- [ ] concurrent edit #task\n");
                fs::write(&edited, text).unwrap();
            }));
        }
    });

    let mut report = report_for(&vault, false);
    execute_standalone(&mut report, &mut hooks);

    assert_eq!(report.exit.outcome, Outcome::Applied, "{:?}", report.exit);
    assert_eq!(attempts.get(), 2, "one rebuild after the stale preflight");
    let text = fs::read_to_string(&source).unwrap();
    assert!(text.contains("concurrent edit"), "edit survives:\n{text}");
    assert!(!text.contains("done one"), "{text}");
    let archive =
        fs::read_to_string(vault.path().join("done/work_done.md")).unwrap();
    assert!(archive.contains("done one"), "{archive}");
    assert_eq!(report.exit.applied_paths.len(), 2);
}

#[test]
fn exhausted_retry_budget_writes_nothing() {
    let vault = fixture("bob-cli-archive-exec-exhausted");
    let scratch = TempDir::new("bob-cli-archive-exec-exhausted-scratch");
    let _env = lock_env(scratch.path());
    let source = vault.path().join("work.md");
    let attempts = Rc::new(Cell::new(0usize));
    let seen = attempts.clone();
    let edited = source.clone();
    let mut hooks = hooks(&scratch.path().join("state"), move |session, n| {
        seen.set(n + 1);
        let edited = edited.clone();
        session.before_preflight = Some(Box::new(move || {
            let mut text = fs::read_to_string(&edited).unwrap();
            text.push_str(&format!("- [ ] racing edit {n} #task\n"));
            fs::write(&edited, text).unwrap();
        }));
    });
    hooks.budget = Duration::from_secs(1);

    let mut report = report_for(&vault, false);
    execute_standalone(&mut report, &mut hooks);

    assert_eq!(report.exit.outcome, Outcome::Failed);
    assert_eq!(report.exit.reason, Some("retry_exhausted"));
    assert_eq!(report.exit_code(), 1);
    assert!(attempts.get() >= 2, "retried at least once");
    assert!(report.exit.applied_paths.is_empty());
    assert!(!vault.path().join("done").exists(), "nothing was written");
    let text = fs::read_to_string(&source).unwrap();
    assert!(text.contains("done one"), "moves were not applied:\n{text}");
    assert!(text.contains("racing edit 0"), "edits preserved:\n{text}");
    let value = json(&report);
    assert_eq!(value["exit"]["reason"], "retry_exhausted");
    assert_eq!(value["git"]["outcome"], "not_run");
}

#[test]
fn partial_apply_stops_then_exact_replay_completes_the_move() {
    let vault = fixture("bob-cli-archive-exec-partial");
    let scratch = TempDir::new("bob-cli-archive-exec-partial-scratch");
    let _env = lock_env(scratch.path());
    let state = scratch.path().join("state");
    let source = vault.path().join("work.md");
    let edited = source.clone();
    // The source changes after the archive is installed but before the source
    // removal: the archive copy lands, the removal is deferred.
    let mut interrupted = hooks(&state, move |session, n| {
        if n != 0 {
            return;
        }
        let edited = edited.clone();
        session.before_replace = Some(Box::new(move |dest| {
            if dest.ends_with("work.md") {
                let mut text = fs::read_to_string(&edited).unwrap();
                text.push_str("trailing concurrent note\n");
                fs::write(&edited, text).unwrap();
            }
        }));
    });

    let mut first = report_for(&vault, false);
    execute_standalone(&mut first, &mut interrupted);

    assert_eq!(first.exit.outcome, Outcome::Failed);
    assert_eq!(first.exit.reason, Some("partial_apply"));
    assert_eq!(first.exit_code(), 1);
    assert_eq!(first.exit.applied_paths, rel(&["done/work_done.md"]));
    assert_eq!(first.exit.deferred_paths, rel(&["work.md"]));
    let recovery = first.exit.recovery_directory.clone().expect("recovery");
    assert!(recovery.exists(), "{}", recovery.display());
    assert_eq!(first.git.outcome, super::super::git::GitOutcome::NotRun);
    let value = json(&first);
    assert_eq!(value["exit"]["reason"], "partial_apply");
    assert_eq!(
        value["exit"]["deferred_paths"],
        serde_json::json!(["work.md"])
    );
    assert_eq!(value["git"]["outcome"], "not_run");
    let archive_after_first =
        fs::read_to_string(vault.path().join("done/work_done.md")).unwrap();
    assert!(archive_after_first.contains("done one"));
    let source_after_first = fs::read_to_string(&source).unwrap();
    assert!(
        source_after_first.contains("done one"),
        "still in the source"
    );

    // Preview of the explicit rerun names both roots as exact replays.
    let mut preview = report_for(&vault, true);
    execute_standalone(&mut preview, &mut plain_hooks(&state));
    let moves = json(&preview)["moves"].clone();
    assert_eq!(
        moves[0]["replays"],
        serde_json::json!([
            {"line": 5, "block_id": "one"},
            {"line": 6, "block_id": null}
        ])
    );
    assert_eq!(moves[0]["writes_archive"], false);

    let mut second = report_for(&vault, false);
    execute_standalone(&mut second, &mut plain_hooks(&state));

    assert_eq!(second.exit.outcome, Outcome::Applied, "{:?}", second.exit);
    assert_eq!(second.exit.applied_paths, rel(&["work.md"]));
    let archive =
        fs::read_to_string(vault.path().join("done/work_done.md")).unwrap();
    assert_eq!(
        archive, archive_after_first,
        "archive is not appended twice"
    );
    assert_eq!(archive.matches("done one #task").count(), 1);
    let source_text = fs::read_to_string(&source).unwrap();
    assert!(!source_text.contains("done one"), "{source_text}");
    assert!(source_text.contains("trailing concurrent note"));

    // A third run has nothing left to do.
    let mut third = report_for(&vault, false);
    execute_standalone(&mut third, &mut plain_hooks(&state));
    assert_eq!(third.exit.outcome, Outcome::Noop);
}

#[test]
fn plan_refusal_is_reported_without_any_write() {
    let vault = fixture("bob-cli-archive-exec-refusal");
    write_file(
        &vault.path().join("done/work_done.md"),
        &archive_note("- [ ] open archived #task ^open\n"),
    );
    let scratch = TempDir::new("bob-cli-archive-exec-refusal-scratch");
    let _env = lock_env(scratch.path());
    let before_source = fs::read(vault.path().join("work.md")).unwrap();
    let before_archive =
        fs::read(vault.path().join("done/work_done.md")).unwrap();

    let mut report = report_for(&vault, false);
    execute_standalone(
        &mut report,
        &mut hooks(&scratch.path().join("state"), |_, _| {
            panic!("a refused plan is never applied")
        }),
    );

    assert_eq!(report.exit.outcome, Outcome::Failed);
    assert_eq!(report.exit.reason, Some("plan_refused"));
    assert_eq!(report.exit_code(), 1);
    assert!(report
        .exit
        .message
        .as_deref()
        .unwrap()
        .contains("open #task"));
    assert_eq!(
        fs::read(vault.path().join("work.md")).unwrap(),
        before_source
    );
    assert_eq!(
        fs::read(vault.path().join("done/work_done.md")).unwrap(),
        before_archive
    );
    let value = json(&report);
    assert_eq!(value["moves"], serde_json::json!([]));
    assert_eq!(value["exit"]["outcome"], "failed");
}

#[test]
fn preview_reports_renames_and_holds() {
    let vault = TempDir::new("bob-cli-archive-exec-renames");
    write_file(
        &vault.path().join("work.md"),
        &area_note(
            "\
- [x] fresh #task ^dup
- [x] parent #task ^held
  - [ ] child #task ^kid
",
        ),
    );
    write_file(
        &vault.path().join("done/work_done.md"),
        &archive_note("- [x] older #task ^dup\n"),
    );
    let scratch = TempDir::new("bob-cli-archive-exec-renames-scratch");

    let mut report = report_for(&vault, true);
    execute_standalone(
        &mut report,
        &mut plain_hooks(&scratch.path().join("state")),
    );

    assert_eq!(report.exit.outcome, Outcome::Preview, "{:?}", report.exit);
    let value = json(&report);
    let moves = value["moves"].as_array().expect("moves");
    assert_eq!(moves.len(), 1);
    assert_eq!(moves[0]["creates_archive"], false);
    assert_eq!(
        moves[0]["id_renames"],
        serde_json::json!([{"from": "dup", "to": "dup-1"}])
    );
    assert_eq!(moves[0]["block_ids"], serde_json::json!(["dup-1"]));
    assert_eq!(
        moves[0]["roots"],
        serde_json::json!([{"line": 5, "block_id": "dup"}])
    );
    assert_eq!(
        value["holds"],
        serde_json::json!([{
            "path": "work.md",
            "line": 6,
            "block_id": "held",
            "reason": "open_subtask"
        }])
    );
    assert!(value["skips"].is_object(), "{value}");
    assert_eq!(value["grandfathered"], serde_json::json!([]));
}

#[test]
fn standalone_lock_timeout_fails_cleanly_then_releases() {
    let vault = fixture("bob-cli-archive-exec-lock");
    let scratch = TempDir::new("bob-cli-archive-exec-lock-scratch");
    let _env = lock_env(scratch.path());
    let state = scratch.path().join("state");
    let held = ob::try_acquire_lock().expect("hold the lock");
    let mut blocked = hooks(&state, |_, _| panic!("lock never acquired"));
    blocked.plan = Box::new(|_, _| panic!("planning needs the lock first"));

    let mut report = report_for(&vault, false);
    execute_standalone(&mut report, &mut blocked);

    assert_eq!(report.exit.outcome, Outcome::Failed);
    assert_eq!(report.exit.reason, Some("lock_timeout"));
    assert_eq!(report.exit_code(), 1);
    assert!(
        report
            .exit
            .message
            .as_deref()
            .unwrap()
            .contains("already active"),
        "{:?}",
        report.exit.message
    );
    assert!(!vault.path().join("done").exists());
    assert_eq!(json(&report)["exit"]["reason"], "lock_timeout");

    drop(held);
    let mut retry = report_for(&vault, false);
    let mut retry_hooks = plain_hooks(&state);
    retry_hooks.lock_wait = Duration::from_secs(5);
    execute_standalone(&mut retry, &mut retry_hooks);
    assert_eq!(retry.exit.outcome, Outcome::Applied, "{:?}", retry.exit);
    assert!(lock_is_released(), "the lock is released again");
}

#[test]
fn nightly_entry_runs_under_a_held_lock_without_reacquiring_it() {
    let vault = fixture("bob-cli-archive-exec-nightly");
    let scratch = TempDir::new("bob-cli-archive-exec-nightly-scratch");
    let _lock = lock_env(scratch.path());
    let _dir = bob_env::TestEnvGuard::set(&[(
        "BOB_DIR",
        Some(vault.path().as_os_str()),
    )]);
    // Nightly owns the lock for its whole run.
    let held = ob::try_acquire_lock().expect("nightly holds the lock");
    let mut hooks = plain_hooks(&scratch.path().join("state"));
    hooks.lock_wait = Duration::from_millis(1);

    let code = run_collection_with(1, &Vec::new(), &mut hooks);

    assert_eq!(code, 0);
    assert!(
        matches!(ob::try_acquire_lock(), Err(ob::LockAcquireError::Contended)),
        "the caller's lock is still held"
    );
    assert!(vault.path().join("done/work_done.md").exists());
    drop(held);
}

#[test]
fn execute_is_the_lock_free_core() {
    let vault = fixture("bob-cli-archive-exec-core");
    let scratch = TempDir::new("bob-cli-archive-exec-core-scratch");
    let mut report = report_for(&vault, false);

    execute(
        &mut report,
        &Vec::new(),
        &mut plain_hooks(&scratch.path().join("state")),
    );

    assert_eq!(report.exit.outcome, Outcome::Applied);
}
