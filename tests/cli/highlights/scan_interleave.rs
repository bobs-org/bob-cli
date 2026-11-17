//! Deterministic adversarial interleavings for the narrow scan lock window.
//!
//! Pause points are handshake-bounded (`BOB_REF_SCAN_TEST_*`): the child
//! writes `<seq>:<stage>` and waits until the go file contains that seq.
//! Subprocesses are reaped on drop. Lock paths and hooks stay inside the
//! temporary vault.

use super::overwrite_recovery;
use crate::support::*;
use fs2::FileExt;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Output, Stdio};
use std::time::{Duration, Instant};

const SCAN_JSON_NOW: &str = "2026-11-17 12:00:00";
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);

struct Handshake {
    ready: PathBuf,
    go: PathBuf,
}

impl Handshake {
    fn new(dir: &Path, name: &str) -> Self {
        let root = dir.join(name);
        fs::create_dir_all(&root).expect("handshake dir");
        Self {
            ready: root.join("ready"),
            go: root.join("go"),
        }
    }
}

struct SpawnedScan {
    child: Option<Child>,
}

impl SpawnedScan {
    fn wait(mut self) -> Output {
        self.child
            .take()
            .expect("spawned scan")
            .wait_with_output()
            .expect("wait for scan")
    }
}

impl Drop for SpawnedScan {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

struct ScanEnv {
    vault: PathBuf,
    state: PathBuf,
    lock: PathBuf,
}

fn wait_until(timeout: Duration, what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if ready() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {what} after {timeout:?}");
}

fn wait_pause(hs: &Handshake, seq: usize) -> String {
    wait_until(HANDSHAKE_TIMEOUT, "scan handshake", || {
        fs::read_to_string(&hs.ready)
            .ok()
            .is_some_and(|body| body.starts_with(&format!("{seq}:")))
    });
    fs::read_to_string(&hs.ready).expect("read ready")
}

fn resume(hs: &Handshake, seq: usize) {
    fs::write(&hs.go, seq.to_string()).expect("write go");
}

fn scan_receipt_path(state: &Path) -> PathBuf {
    state.join("bob-cli/ref/last-scan.json")
}

fn open_lock_file(path: &Path) -> std::fs::File {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("lock dir");
    }
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .unwrap_or_else(|error| panic!("open lock {}: {error}", path.display()))
}

fn hold_exclusive_lock(path: &Path) -> std::fs::File {
    let file = open_lock_file(path);
    wait_until(
        HANDSHAKE_TIMEOUT,
        &format!("exclusive lock {}", path.display()),
        || file.try_lock_exclusive().is_ok(),
    );
    file
}

fn hold_scan_writer_lock(state: &Path) -> std::fs::File {
    hold_exclusive_lock(&state.join("bob-cli/ref/scan.lock"))
}

fn hold_vault_maintenance_lock(path: &Path) -> std::fs::File {
    hold_exclusive_lock(path)
}

fn try_lock_exclusive(path: &Path) -> std::io::Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    file.try_lock_exclusive()?;
    Ok(file)
}

fn scan_json_stdout(output: &Output) -> Value {
    let text = stdout(output);
    assert_eq!(
        text.lines().count(),
        1,
        "JSON stdout must be exactly one line:\n{}",
        format_output(output)
    );
    serde_json::from_str(text.trim()).expect("stdout must parse as JSON")
}

fn write_marker_pdf(path: &Path, title: &str) {
    write_highlights_pdf(
        path,
        &format!("- status: wip\n- parent: obsidian\n- title: {title}\n"),
    );
}

fn apply_scan_env<'a>(
    command: &'a mut std::process::Command,
    env: &ScanEnv,
) -> &'a mut std::process::Command {
    command
        .env("BOB_DIR", &env.vault)
        .env("BOB_NOW", SCAN_JSON_NOW)
        .env("XDG_STATE_HOME", &env.state)
        .env("BOB_VAULT_SYNC_LOCK_FILE", &env.lock)
        .env("BOB_REF_SCAN_LOCK_WAIT_SECONDS", "0")
}

fn run_scan(env: &ScanEnv, extra: &[&str]) -> Output {
    let mut command = bob_command();
    command.arg("ref").arg("scan");
    for arg in extra {
        command.arg(arg);
    }
    apply_scan_env(&mut command, env)
        .output()
        .expect("run ref scan")
}

fn spawn_scan(
    env: &ScanEnv,
    extra: &[&str],
    hs: &Handshake,
    pause: &str,
    pause_every: bool,
    plan_log: Option<&Path>,
) -> SpawnedScan {
    let mut command = bob_command();
    command.arg("ref").arg("scan");
    for arg in extra {
        command.arg(arg);
    }
    apply_scan_env(&mut command, env)
        .env("BOB_REF_SCAN_TEST_PAUSE", pause)
        .env("BOB_REF_SCAN_TEST_READY", &hs.ready)
        .env("BOB_REF_SCAN_TEST_GO", &hs.go)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if pause_every {
        command.env("BOB_REF_SCAN_TEST_PAUSE_EVERY", "1");
    }
    if let Some(log) = plan_log {
        command.env("BOB_REF_SCAN_TEST_PLAN_LOG", log);
    }
    SpawnedScan {
        child: Some(command.spawn().expect("spawn ref scan")),
    }
}

fn assert_no_lock_wait(output: &Output) {
    let err = stderr(output);
    assert!(
        !err.contains("waiting for another bob ref scan to finish")
            && !err.contains("waiting for another vault maintenance run"),
        "scan must not wait on either lock:\n{}",
        format_output(output)
    );
}

fn write_textbundle_with_asset(pdf: &Path, title: &str, asset_bytes: &[u8]) {
    write_textbundle(pdf, title, asset_bytes, "text.md", None);
}

fn write_textbundle(
    pdf: &Path,
    title: &str,
    asset_bytes: &[u8],
    text_file: &str,
    comment: Option<&str>,
) {
    write_marker_pdf(pdf, title);
    let bundle = pdf.with_extension("textbundle");
    let sidecar = bundle.join(text_file);
    let asset = bundle.join("assets/figure.png");
    let comment_block = comment
        .map(|text| format!("\nComment: {text}.\n"))
        .unwrap_or_default();
    write_file(
        &sidecar,
        &format!(
            "\
## Page 1

- status: wip
- parent: obsidian

---

![{title} figure](assets/figure.png)
{comment_block}"
        ),
    );
    fs::create_dir_all(asset.parent().expect("asset parent"))
        .expect("asset dir");
    fs::write(&asset, asset_bytes).expect("write asset");
}

fn spawn_scan_extra(
    env: &ScanEnv,
    extra: &[&str],
    hs: &Handshake,
    pause: &str,
    extra_env: &[(&str, &std::ffi::OsStr)],
) -> SpawnedScan {
    let mut command = bob_command();
    command.arg("ref").arg("scan");
    for arg in extra {
        command.arg(arg);
    }
    apply_scan_env(&mut command, env)
        .env("BOB_REF_SCAN_TEST_PAUSE", pause)
        .env("BOB_REF_SCAN_TEST_READY", &hs.ready)
        .env("BOB_REF_SCAN_TEST_GO", &hs.go)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in extra_env {
        command.env(key, value);
    }
    SpawnedScan {
        child: Some(command.spawn().expect("spawn ref scan")),
    }
}

fn edit_at_pause(
    env: &ScanEnv,
    temp: &Path,
    pause: &str,
    extra_env: &[(&str, &std::ffi::OsStr)],
    edit: impl FnOnce(),
) -> std::process::Output {
    let hs = Handshake::new(temp, "hs");
    let child = spawn_scan_extra(env, &["-f", "json"], &hs, pause, extra_env);
    wait_pause(&hs, 1);
    edit();
    resume(&hs, 1);
    child.wait()
}

fn assert_inputs_changed_or_imported(
    output: &std::process::Output,
    note: &Path,
    before: &str,
    imported_needle: &str,
) {
    assert!(
        output.status.code() == Some(0) || output.status.code() == Some(1),
        "handshake mutation must replan or refuse:\n{}",
        format_output(output)
    );
    let document = scan_json_stdout(output);
    if document.get("error").is_some() {
        assert_eq!(document["error"]["code"], "scan_failed");
        assert!(
            document["error"]["message"]
                .as_str()
                .unwrap_or("")
                .contains("scan inputs changed"),
            "{}",
            format_output(output)
        );
        assert_eq!(
            fs::read_to_string(note).expect("note after refusal"),
            before,
            "scan_failed must not mutate the note"
        );
    } else {
        assert_eq!(output.status.code(), Some(0), "{}", format_output(output));
        let after = fs::read_to_string(note).expect("note after import");
        assert!(
            after.contains(imported_needle),
            "replan must import current bytes:\n{}\nnote:\n{after}",
            format_output(output)
        );
        assert_ne!(after, before, "imported note must change");
    }
}

// --- 1. lock-free no-work under held locks ---

#[test]
fn no_work_scan_succeeds_while_each_lock_is_held() {
    let temp = TempDir::new("bob-cli-scan-interleave-nowork-locks");
    let env = ScanEnv {
        vault: temp.path().join("vault"),
        state: temp.path().join("state"),
        lock: temp.path().join("bob_sync.lock"),
    };
    write_marker_pdf(&env.vault.join("lib/chat/stable.pdf"), "Stable");
    assert_success(&run_scan(&env, &[]));
    let receipt_before =
        fs::read(scan_receipt_path(&env.state)).expect("initial receipt");

    let cases: &[(&str, bool, bool)] = &[
        ("both", true, true),
        ("scan-only", true, false),
        ("maint-only", false, true),
    ];
    for (label, hold_scan, hold_maint) in cases {
        let _scan = hold_scan.then(|| hold_scan_writer_lock(&env.state));
        let _maint = hold_maint.then(|| hold_vault_maintenance_lock(&env.lock));
        let output = run_scan(&env, &["-f", "json"]);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{label} no-work json must succeed:\n{}",
            format_output(&output)
        );
        assert_no_lock_wait(&output);
        let document = scan_json_stdout(&output);
        assert_eq!(document["ok"], true, "{label}");
        assert_eq!(document["summary"]["unchanged"], 1, "{label}");
        assert_eq!(
            fs::read(scan_receipt_path(&env.state)).expect("receipt"),
            receipt_before,
            "{label} must leave the sentinel receipt untouched"
        );
    }
}

#[test]
fn no_work_empty_library_and_annotated_assets_ignore_held_locks() {
    let temp = TempDir::new("bob-cli-scan-interleave-nowork-shapes");
    let empty = ScanEnv {
        vault: temp.path().join("empty"),
        state: temp.path().join("empty-state"),
        lock: temp.path().join("empty.lock"),
    };
    fs::create_dir_all(empty.vault.join("lib")).expect("empty lib");
    let _scan = hold_scan_writer_lock(&empty.state);
    let _maint = hold_vault_maintenance_lock(&empty.lock);
    let output = run_scan(&empty, &["-f", "json"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "empty library no-work:\n{}",
        format_output(&output)
    );
    assert_no_lock_wait(&output);
    let document = scan_json_stdout(&output);
    assert_eq!(document["ok"], true);
    assert_eq!(document["summary"]["pdfs"], 0);
    assert!(
        !scan_receipt_path(&empty.state).exists(),
        "empty no-work must not publish a receipt"
    );

    let annotated = ScanEnv {
        vault: temp.path().join("annotated"),
        state: temp.path().join("annotated-state"),
        lock: temp.path().join("annotated.lock"),
    };
    let pdf = annotated.vault.join("lib/books/figures.pdf");
    write_textbundle_with_asset(&pdf, "Figures", b"synthetic png bytes");
    assert_success(&run_scan(&annotated, &[]));
    let note = annotated.vault.join("ref/books/figures.md");
    let assets = annotated.vault.join("ref/books/figures.assets");
    let note_before = fs::read(&note).expect("note");
    let receipt_before =
        fs::read(scan_receipt_path(&annotated.state)).expect("receipt");
    assert!(assets.is_dir(), "first scan copies image assets");
    let _scan = hold_scan_writer_lock(&annotated.state);
    let _maint = hold_vault_maintenance_lock(&annotated.lock);
    let output = run_scan(&annotated, &["-f", "json"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "annotated no-work:\n{}",
        format_output(&output)
    );
    assert_no_lock_wait(&output);
    let document = scan_json_stdout(&output);
    assert_eq!(document["summary"]["unchanged"], 1);
    assert_eq!(fs::read(&note).expect("note after"), note_before);
    assert_eq!(
        fs::read(scan_receipt_path(&annotated.state)).expect("receipt after"),
        receipt_before
    );
}

// --- 2. planning stays unlocked; parser does not rerun under locks ---

#[test]
fn planning_pause_lets_maintenance_writer_finish_without_replanning() {
    let temp = TempDir::new("bob-cli-scan-interleave-planning");
    let env = ScanEnv {
        vault: temp.path().join("vault"),
        state: temp.path().join("state"),
        lock: temp.path().join("bob_sync.lock"),
    };
    write_marker_pdf(&env.vault.join("lib/chat/stable.pdf"), "Stable");
    write_marker_pdf(&env.vault.join("lib/chat/fresh.pdf"), "Fresh");
    assert_success(
        &bob_command()
            .arg("ref")
            .arg("sync")
            .arg(env.vault.join("lib/chat/stable.pdf"))
            .env("BOB_DIR", &env.vault)
            .env("XDG_STATE_HOME", &env.state)
            .env("BOB_VAULT_SYNC_LOCK_FILE", &env.lock)
            .output()
            .expect("converge stable pdf"),
    );
    let hs = Handshake::new(temp.path(), "hs");
    let plan_log = temp.path().join("plan.log");
    let sentinel = temp.path().join("maint-writer");
    let child = spawn_scan(
        &env,
        &["-f", "json"],
        &hs,
        "planning",
        false,
        Some(&plan_log),
    );
    wait_pause(&hs, 1);
    {
        let _held = hold_vault_maintenance_lock(&env.lock);
        fs::write(&sentinel, "wrote").expect("maintenance writer finished");
    }
    resume(&hs, 1);
    let output = child.wait();
    assert_eq!(
        output.status.code(),
        Some(0),
        "scan after unlocked planning:\n{}",
        format_output(&output)
    );
    assert!(sentinel.is_file(), "maintenance writer completed mid-plan");
    assert!(env.vault.join("ref/chat/fresh.md").is_file());
    let log = fs::read_to_string(&plan_log).expect("plan log");
    assert!(
        log.lines().any(|line| line.starts_with("locked=0 count=2")),
        "unlocked planning must cover both PDFs:\n{log}"
    );
    assert!(
        log.lines().all(|line| line.starts_with("locked=0")),
        "existing library PDFs must not be parsed again under the locks:\n{log}"
    );
}

// --- 3. lock_busy / scan_busy with real work: no mutation, no receipt ---

fn pending_intake_env(prefix: &str) -> (TempDir, ScanEnv, PathBuf, PathBuf) {
    let temp = TempDir::new(prefix);
    let env = ScanEnv {
        vault: temp.path().join("vault"),
        state: temp.path().join("state"),
        lock: temp.path().join("bob_sync.lock"),
    };
    let source = env.vault.join("xlib/chat/queued.pdf");
    write_marker_pdf(&source, "Queued");
    let note = env.vault.join("ref/chat/queued.md");
    (temp, env, source, note)
}

#[test]
fn pending_work_lock_busy_beats_scan_busy_and_writes_nothing() {
    let (_temp, env, source, note) =
        pending_intake_env("bob-cli-scan-interleave-lock-order");
    let _scan = hold_scan_writer_lock(&env.state);
    let _maint = hold_vault_maintenance_lock(&env.lock);
    let output = run_scan(&env, &["-f", "json"]);
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let document = scan_json_stdout(&output);
    assert_eq!(document["error"]["code"], "lock_busy");
    assert_eq!(
        document["error"]["hint"],
        "vault-sync, nightly, or rename held the maintenance lock; wait for it to finish, then scan again"
    );
    assert!(
        document["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains(path_str(&env.lock)),
        "lock_busy names the lock path:\n{}",
        format_output(&output)
    );
    assert!(source.is_file(), "intake must not move on lock_busy");
    assert!(!note.exists(), "note must not be written on lock_busy");
    assert!(
        !scan_receipt_path(&env.state).exists(),
        "lock_busy must not write a receipt"
    );
}

#[test]
fn pending_work_scan_busy_writes_nothing() {
    let (_temp, env, source, note) =
        pending_intake_env("bob-cli-scan-interleave-scan-busy");
    let _scan = hold_scan_writer_lock(&env.state);
    let output = run_scan(&env, &["-f", "json"]);
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let document = scan_json_stdout(&output);
    assert_eq!(document["error"]["code"], "scan_busy");
    assert_eq!(
        document["error"]["message"],
        "another bob ref scan is still running"
    );
    assert_eq!(
        document["error"]["hint"],
        "wait for it to finish, then scan again"
    );
    assert!(source.is_file());
    assert!(!note.exists());
    assert!(!scan_receipt_path(&env.state).exists());
}

#[test]
fn pending_work_human_lock_errors_match_json() {
    let (_temp, env, source, _note) =
        pending_intake_env("bob-cli-scan-interleave-human-busy");
    let _maint = hold_vault_maintenance_lock(&env.lock);
    let output = run_scan(&env, &[]);
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let err = stderr(&output);
    assert!(
        err.contains("waiting for another vault maintenance run…")
            && err.contains("could not acquire the vault maintenance lock")
            && err.contains(path_str(&env.lock)),
        "human lock_busy shape:\n{}",
        format_output(&output)
    );
    assert!(source.is_file());
    assert!(!scan_receipt_path(&env.state).exists());
}

// --- 4. stale inputs between prepare and acquire ---

#[test]
fn stale_inputs_after_prepare_replans_or_refuses() {
    let temp = TempDir::new("bob-cli-scan-interleave-stale-inputs");
    let env = ScanEnv {
        vault: temp.path().join("vault"),
        state: temp.path().join("state"),
        lock: temp.path().join("bob_sync.lock"),
    };
    write_area_note(&env.vault.join("obsidian.md"), "Obsidian");
    let pdf = env.vault.join("lib/chat/live.pdf");
    write_textbundle_with_asset(&pdf, "Live", b"asset-v1");
    assert_success(&run_scan(&env, &[]));
    let note = env.vault.join("ref/chat/live.md");
    let original = fs::read_to_string(&note).expect("note");
    write_file(&note, &original.replace("title: Live", "title: Live Edit"));

    let hs = Handshake::new(temp.path(), "hs");
    let hook_log = temp.path().join("hook.log");
    let script = temp.path().join("pre-scan");
    write_executable(
        &script,
        &format!(
            "#!/bin/sh\nprintf hook >> {}\n",
            shell_single_quote(path_str(&hook_log))
        ),
    );
    let config = temp.path().join("config.yml");
    write_file(
        &config,
        &format!(
            "highlights:\n  pre_scan_hook: {}\n",
            shell_single_quote(path_str(&script))
        ),
    );

    let child = {
        let mut command = bob_command();
        command.arg("ref").arg("scan").arg("-f").arg("json");
        apply_scan_env(&mut command, &env)
            .env("BOB_CONFIG_FILE", &config)
            .env("BOB_REF_SCAN_TEST_PAUSE", "after_prepare")
            .env("BOB_REF_SCAN_TEST_READY", &hs.ready)
            .env("BOB_REF_SCAN_TEST_GO", &hs.go)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        SpawnedScan {
            child: Some(command.spawn().expect("spawn")),
        }
    };
    wait_pause(&hs, 1);
    write_marker_pdf(&pdf, "Live annotations changed");
    write_file(&note, &format!("{original}\n# extra heading\n"));
    write_file(
        &pdf.with_extension("textbundle").join("text.md"),
        "\
## Page 1

- status: wip
- parent: obsidian

---

> sidecar changed
",
    );
    write_marker_pdf(&env.vault.join("lib/chat/added.pdf"), "Added");
    let parent = fs::read_to_string(env.vault.join("obsidian.md"))
        .unwrap_or_else(|_| {
            "---\ntype: \"[[area]]\"\n---\n\n# Obsidian\n".into()
        });
    if let Some(task) = parent.lines().find(|line| line.contains("#ref")) {
        write_file(
            &env.vault.join("elsewhere.md"),
            &format!("---\ntype: \"[[area]]\"\n---\n\n# Elsewhere\n\n{task}\n"),
        );
        let stripped = parent
            .lines()
            .filter(|line| !line.contains("#ref"))
            .collect::<Vec<_>>()
            .join("\n");
        write_file(&env.vault.join("obsidian.md"), &format!("{stripped}\n"));
    }
    resume(&hs, 1);
    let output = child.wait();
    assert!(
        output.status.code() == Some(0) || output.status.code() == Some(1),
        "stale inputs must replan or refuse, never hang:\n{}",
        format_output(&output)
    );
    let document = scan_json_stdout(&output);
    if document.get("error").is_some() {
        assert_eq!(
            document["error"]["code"],
            "scan_failed",
            "{}",
            format_output(&output)
        );
        assert!(
            document["error"]["message"]
                .as_str()
                .unwrap_or("")
                .contains("scan inputs changed"),
            "persistent churn names a rerun:\n{}",
            format_output(&output)
        );
    } else {
        assert_eq!(
            document["ok"],
            output.status.success(),
            "report envelope ok must match the exit:\n{}",
            format_output(&output)
        );
        assert!(
            env.vault.join("ref/chat/live.md").is_file(),
            "fresh plan must keep a well-formed note:\n{}",
            format_output(&output)
        );
        let created = document["summary"]["created"].as_u64().unwrap_or(0);
        let updated = document["summary"]["updated"].as_u64().unwrap_or(0);
        let failures = document["summary"]["failures"].as_u64().unwrap_or(0);
        assert!(
            created + updated + failures > 0,
            "edits must not be classified stale no-work:\n{}",
            format_output(&output)
        );
    }
    let hook = fs::read_to_string(&hook_log).unwrap_or_default();
    assert_eq!(hook.matches("hook").count(), 1, "hook runs once: {hook}");
}

#[test]
fn repeated_churn_after_prepare_terminates() {
    let temp = TempDir::new("bob-cli-scan-interleave-churn");
    let env = ScanEnv {
        vault: temp.path().join("vault"),
        state: temp.path().join("state"),
        lock: temp.path().join("bob_sync.lock"),
    };
    write_marker_pdf(&env.vault.join("lib/chat/churn.pdf"), "Churn");
    let hs = Handshake::new(temp.path(), "hs");
    let child =
        spawn_scan(&env, &["-f", "json"], &hs, "before_lock", true, None);
    wait_pause(&hs, 1);
    write_marker_pdf(&env.vault.join("lib/chat/churn.pdf"), "Churn v2");
    resume(&hs, 1);
    wait_pause(&hs, 2);
    write_marker_pdf(&env.vault.join("lib/chat/churn.pdf"), "Churn v3");
    resume(&hs, 2);
    let output = child.wait();
    assert_eq!(
        output.status.code(),
        Some(1),
        "repeated churn must terminate:\n{}",
        format_output(&output)
    );
    let document = scan_json_stdout(&output);
    assert_eq!(document["error"]["code"], "scan_failed");
    assert!(
        document["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("rerun bob ref scan"),
        "{}",
        format_output(&output)
    );
    assert!(
        !scan_receipt_path(&env.state).exists(),
        "abandoned retry must not publish an intermediate receipt"
    );
}

// --- 5. two scans, one intake; paired note stays locked ---

#[test]
fn two_scans_one_queued_pdf_only_one_intakes() {
    let temp = TempDir::new("bob-cli-scan-interleave-two-scans");
    let env = ScanEnv {
        vault: temp.path().join("vault"),
        state: temp.path().join("state"),
        lock: temp.path().join("bob_sync.lock"),
    };
    let source = env.vault.join("xlib/chat/race.pdf");
    write_marker_pdf(&source, "Race");
    write_file(
        &source.with_extension("md"),
        "\
## Page 1

- status: wip
- parent: obsidian

---

> sidecar quote.
",
    );
    write_file(&source.with_extension("mp3"), "fake-mp3");
    let hs_a = Handshake::new(temp.path(), "a");
    let hs_b = Handshake::new(temp.path(), "b");
    let a =
        spawn_scan(&env, &["-f", "json"], &hs_a, "after_prepare", false, None);
    let b =
        spawn_scan(&env, &["-f", "json"], &hs_b, "after_prepare", false, None);
    wait_pause(&hs_a, 1);
    wait_pause(&hs_b, 1);
    resume(&hs_a, 1);
    let out_a = a.wait();
    assert_eq!(
        out_a.status.code(),
        Some(0),
        "first scan:\n{}",
        format_output(&out_a)
    );
    resume(&hs_b, 1);
    let out_b = b.wait();
    assert_eq!(
        out_b.status.code(),
        Some(0),
        "second scan:\n{}",
        format_output(&out_b)
    );
    let dest = env.vault.join("lib/chat/race.pdf");
    let note = env.vault.join("ref/chat/race.md");
    assert!(dest.is_file());
    assert!(dest.with_extension("md").is_file());
    assert!(dest.with_extension("mp3").is_file());
    assert!(note.is_file());
    assert!(!source.exists());
    let doc_a = scan_json_stdout(&out_a);
    let doc_b = scan_json_stdout(&out_b);
    let created_a = doc_a["summary"]["created"].as_u64().unwrap_or(0);
    let created_b = doc_b["summary"]["created"].as_u64().unwrap_or(0);
    assert_eq!(
        created_a + created_b,
        1,
        "exactly one scan creates the note:\nA {}\nB {}",
        format_output(&out_a),
        format_output(&out_b)
    );
}

#[test]
fn two_scans_late_audio_moves_once() {
    let temp = TempDir::new("bob-cli-scan-interleave-late-audio");
    let env = ScanEnv {
        vault: temp.path().join("vault"),
        state: temp.path().join("state"),
        lock: temp.path().join("bob_sync.lock"),
    };
    write_marker_pdf(&env.vault.join("lib/chat/listen.pdf"), "Listen");
    assert_success(&run_scan(&env, &[]));
    write_file(&env.vault.join("xlib/chat/listen.mp3"), "late-audio");
    let hs_a = Handshake::new(temp.path(), "a");
    let hs_b = Handshake::new(temp.path(), "b");
    let a =
        spawn_scan(&env, &["-f", "json"], &hs_a, "after_prepare", false, None);
    let b =
        spawn_scan(&env, &["-f", "json"], &hs_b, "after_prepare", false, None);
    wait_pause(&hs_a, 1);
    wait_pause(&hs_b, 1);
    resume(&hs_a, 1);
    let out_a = a.wait();
    resume(&hs_b, 1);
    let out_b = b.wait();
    assert_eq!(out_a.status.code(), Some(0), "{}", format_output(&out_a));
    assert_eq!(out_b.status.code(), Some(0), "{}", format_output(&out_b));
    assert!(env.vault.join("lib/chat/listen.mp3").is_file());
    assert!(!env.vault.join("xlib/chat/listen.mp3").exists());
}

#[test]
fn intake_move_holds_maintenance_lock_until_note_exists() {
    let temp = TempDir::new("bob-cli-scan-interleave-paired-note");
    let env = ScanEnv {
        vault: temp.path().join("vault"),
        state: temp.path().join("state"),
        lock: temp.path().join("bob_sync.lock"),
    };
    write_marker_pdf(&env.vault.join("xlib/chat/pair.pdf"), "Pair");
    let hs = Handshake::new(temp.path(), "hs");
    let child = spawn_scan(
        &env,
        &["-f", "json"],
        &hs,
        "after_intake_move",
        false,
        None,
    );
    wait_pause(&hs, 1);
    assert!(
        env.vault.join("lib/chat/pair.pdf").is_file(),
        "PDF has moved"
    );
    assert!(
        !env.vault.join("ref/chat/pair.md").exists(),
        "note is not filed yet"
    );
    assert!(
        try_lock_exclusive(&env.lock).is_err(),
        "vault-sync-like taker cannot enter until the paired note exists"
    );
    resume(&hs, 1);
    let output = child.wait();
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    assert!(env.vault.join("ref/chat/pair.md").is_file());
}

// --- 6. overwrite recovery before hook; intent during prepare ---

#[test]
fn recovery_runs_under_locks_before_hook() {
    let (temp, vault, state) =
        overwrite_recovery::fixture("bob-cli-scan-interleave-recovery-hook");
    let base = overwrite_recovery::head(&vault);
    fs::write(
        vault.join("ref/blogs/half-written.md"),
        "---\nid: half-written\n---\nCRASHED\n",
    )
    .expect("dirty note");
    overwrite_recovery::write_intent(
        &state,
        &vault,
        &base,
        None,
        &["ref/blogs/half-written.md"],
        &[],
    );
    let hook_log = temp.path().join("hook.log");
    let script = temp.path().join("pre-scan");
    write_executable(
        &script,
        &format!(
            "#!/bin/sh\n\
             if grep -q CRASHED ref/blogs/half-written.md; then printf TOO_EARLY >> {log}; fi\n\
             printf HOOK >> {log}\n",
            log = shell_single_quote(path_str(&hook_log))
        ),
    );
    let config = temp.path().join("config.yml");
    write_file(
        &config,
        &format!(
            "highlights:\n  pre_scan_hook: {}\n",
            shell_single_quote(path_str(&script))
        ),
    );
    let lock = temp.path().join("bob_sync.lock");
    let output = bob_command()
        .arg("ref")
        .arg("scan")
        .arg("-f")
        .arg("json")
        .env("BOB_DIR", &vault)
        .env("BOB_CONFIG_FILE", &config)
        .env("XDG_STATE_HOME", &state)
        .env("BOB_VAULT_SYNC_LOCK_FILE", &lock)
        .env("BOB_NOW", SCAN_JSON_NOW)
        .output()
        .expect("scan with pending recovery");
    assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
    assert!(
        stderr(&output)
            .contains("recovered: rolled back an interrupted overwrite"),
        "{}",
        format_output(&output)
    );
    let hook = fs::read_to_string(&hook_log).expect("hook log");
    assert!(
        hook.contains("HOOK") && !hook.contains("TOO_EARLY"),
        "recovery must finish before the hook runs: {hook}"
    );
}

#[test]
fn intent_arriving_during_prepare_invalidates_the_plan() {
    let (temp, vault, state) =
        overwrite_recovery::fixture("bob-cli-scan-interleave-intent-during");
    let base = overwrite_recovery::head(&vault);
    write_marker_pdf(&vault.join("lib/blogs/fresh.pdf"), "Fresh");
    let env = ScanEnv {
        vault: vault.clone(),
        state: state.clone(),
        lock: temp.path().join("bob_sync.lock"),
    };
    let hs = Handshake::new(temp.path(), "hs");
    let child =
        spawn_scan(&env, &["-f", "json"], &hs, "after_prepare", false, None);
    wait_pause(&hs, 1);
    fs::write(
        vault.join("ref/blogs/half-written.md"),
        "---\nid: half-written\n---\nCRASHED\n",
    )
    .expect("crash the note");
    overwrite_recovery::write_intent(
        &state,
        &vault,
        &base,
        None,
        &["ref/blogs/half-written.md"],
        &[],
    );
    resume(&hs, 1);
    let output = child.wait();
    assert_eq!(
        output.status.code(),
        Some(0),
        "invalidated plan recovers then continues:\n{}",
        format_output(&output)
    );
    let note = fs::read_to_string(vault.join("ref/blogs/half-written.md"))
        .expect("restored note");
    assert!(
        note.contains("body"),
        "intent arriving during prepare must recover: {note}"
    );
    assert!(
        !overwrite_recovery::intent_path(&state, &vault).exists(),
        "intent is cleared"
    );
    assert!(vault.join("ref/blogs/fresh.md").is_file());
}

// --- 7. human/json/receipt agreement ---

#[test]
fn human_verbose_and_json_no_work_agree_with_chatty_hook() {
    let temp = TempDir::new("bob-cli-scan-interleave-modes");
    let env = ScanEnv {
        vault: temp.path().join("vault"),
        state: temp.path().join("state"),
        lock: temp.path().join("bob_sync.lock"),
    };
    write_marker_pdf(&env.vault.join("lib/chat/stable.pdf"), "Stable");
    assert_success(&run_scan(&env, &[]));
    let receipt_before =
        fs::read(scan_receipt_path(&env.state)).expect("receipt");
    let script = temp.path().join("pre-scan");
    write_executable(
        &script,
        "#!/bin/sh\nprintf 'hook stdout line\\n'\nprintf 'hook stderr line\\n' >&2\n",
    );
    let config = temp.path().join("config.yml");
    write_file(
        &config,
        &format!(
            "highlights:\n  pre_scan_hook: {}\n",
            shell_single_quote(path_str(&script))
        ),
    );
    let _scan = hold_scan_writer_lock(&env.state);
    let _maint = hold_vault_maintenance_lock(&env.lock);

    let json = {
        let mut command = bob_command();
        command.arg("ref").arg("scan").arg("-f").arg("json");
        apply_scan_env(&mut command, &env)
            .env("BOB_CONFIG_FILE", &config)
            .output()
            .expect("json")
    };
    assert_eq!(json.status.code(), Some(0), "{}", format_output(&json));
    assert_no_lock_wait(&json);
    let document = scan_json_stdout(&json);
    assert_eq!(document["ok"], true);
    assert_eq!(document["summary"]["unchanged"], 1);
    assert!(
        !stdout(&json).contains("hook stdout line"),
        "chatty hook must not corrupt JSON stdout:\n{}",
        format_output(&json)
    );
    assert!(
        stderr(&json).contains("hook stdout line")
            && stderr(&json).contains("hook stderr line"),
        "{}",
        format_output(&json)
    );

    let human = {
        let mut command = bob_command();
        command.arg("ref").arg("scan").arg("--verbose");
        apply_scan_env(&mut command, &env)
            .env("BOB_CONFIG_FILE", &config)
            .output()
            .expect("human")
    };
    assert_eq!(human.status.code(), Some(0), "{}", format_output(&human));
    assert_no_lock_wait(&human);
    assert!(
        stdout(&human).contains("notes_unchanged: 1"),
        "{}",
        format_output(&human)
    );
    assert_eq!(
        fs::read(scan_receipt_path(&env.state)).expect("receipt after"),
        receipt_before,
        "no-work modes must not replace the receipt"
    );
}

fn plant_clash_destination(env: &ScanEnv) {
    write_marker_pdf(&env.vault.join("lib/chat/clash.pdf"), "Existing");
}

#[test]
fn prelock_intake_collision_writes_no_receipt() {
    let temp = TempDir::new("bob-cli-scan-interleave-prelock-receipt");
    let env = ScanEnv {
        vault: temp.path().join("vault"),
        state: temp.path().join("state"),
        lock: temp.path().join("bob_sync.lock"),
    };
    write_marker_pdf(&env.vault.join("xlib/chat/clash.pdf"), "Clash");
    let hs = Handshake::new(temp.path(), "hs");
    let child =
        spawn_scan(&env, &["-f", "json"], &hs, "after_prepare", false, None);
    wait_pause(&hs, 1);
    plant_clash_destination(&env);
    resume(&hs, 1);
    let output = child.wait();
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    assert!(
        !scan_receipt_path(&env.state).exists(),
        "a destination planted before lock acquisition is prelock:\n{}",
        format_output(&output)
    );
    let document = scan_json_stdout(&output);
    assert_eq!(document["ok"], false);
    assert_eq!(
        document["error"]["code"],
        "intake_collision",
        "{}",
        format_output(&output)
    );
}

#[test]
fn human_prelock_intake_collision_writes_no_receipt() {
    let temp = TempDir::new("bob-cli-scan-interleave-human-prelock");
    let env = ScanEnv {
        vault: temp.path().join("vault"),
        state: temp.path().join("state"),
        lock: temp.path().join("bob_sync.lock"),
    };
    write_marker_pdf(&env.vault.join("xlib/chat/clash.pdf"), "Clash");
    let hs = Handshake::new(temp.path(), "hs");
    let child = spawn_scan(&env, &[], &hs, "after_prepare", false, None);
    wait_pause(&hs, 1);
    plant_clash_destination(&env);
    resume(&hs, 1);
    let output = child.wait();
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    assert!(
        !scan_receipt_path(&env.state).exists(),
        "human prelock collision must not write a receipt:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("xlib intake collision"),
        "{}",
        format_output(&output)
    );
}

#[test]
fn post_lock_intake_collision_writes_error_receipt() {
    let temp = TempDir::new("bob-cli-scan-interleave-postlock-receipt");
    let env = ScanEnv {
        vault: temp.path().join("vault"),
        state: temp.path().join("state"),
        lock: temp.path().join("bob_sync.lock"),
    };
    write_marker_pdf(&env.vault.join("xlib/chat/clash.pdf"), "Clash");
    let hs = Handshake::new(temp.path(), "hs");
    let child =
        spawn_scan(&env, &["-f", "json"], &hs, "before_intake", false, None);
    wait_pause(&hs, 1);
    plant_clash_destination(&env);
    resume(&hs, 1);
    let output = child.wait();
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let text = stdout(&output);
    let receipt = fs::read_to_string(scan_receipt_path(&env.state))
        .expect("post-lock hard failure writes a receipt");
    assert_eq!(
        receipt, text,
        "JSON stdout and the post-lock error receipt must match"
    );
    let document: Value = serde_json::from_str(text.trim()).expect("json");
    assert_eq!(document["ok"], false);
    assert_eq!(document["error"]["code"], "intake_collision");
}

#[test]
fn human_post_lock_intake_collision_writes_error_receipt() {
    let temp = TempDir::new("bob-cli-scan-interleave-human-postlock");
    let env = ScanEnv {
        vault: temp.path().join("vault"),
        state: temp.path().join("state"),
        lock: temp.path().join("bob_sync.lock"),
    };
    write_marker_pdf(&env.vault.join("xlib/chat/clash.pdf"), "Clash");
    let hs = Handshake::new(temp.path(), "hs");
    let child = spawn_scan(&env, &[], &hs, "before_intake", false, None);
    wait_pause(&hs, 1);
    plant_clash_destination(&env);
    resume(&hs, 1);
    let output = child.wait();
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let receipt: Value = serde_json::from_str(
        &fs::read_to_string(scan_receipt_path(&env.state))
            .expect("human post-lock failure writes a receipt"),
    )
    .expect("receipt json");
    assert_eq!(receipt["ok"], false);
    assert_eq!(receipt["error"]["code"], "intake_collision");
    assert!(
        stderr(&output).contains("xlib intake collision"),
        "{}",
        format_output(&output)
    );
}

// --- Landing repairs: coherent evidence, combined collisions, receipts ---

#[test]
fn pdf_created_during_planning_replans_with_both() {
    // A PDF arriving while the `planning` handshake is paused must not be
    // omitted: the pre/post read-set comparison must mark the old plan
    // stale so the bounded retry rebuilds with current data.
    let temp = TempDir::new("bob-cli-scan-interleave-planning-membership");
    let env = ScanEnv {
        vault: temp.path().join("vault"),
        state: temp.path().join("state"),
        lock: temp.path().join("bob_sync.lock"),
    };
    write_marker_pdf(&env.vault.join("lib/blogs/first.pdf"), "First");
    let hs = Handshake::new(temp.path(), "hs");
    let child = spawn_scan(&env, &["-f", "json"], &hs, "planning", false, None);
    wait_pause(&hs, 1);
    write_marker_pdf(&env.vault.join("lib/blogs/second.pdf"), "Second");
    resume(&hs, 1);
    let output = child.wait();
    assert_eq!(
        output.status.code(),
        Some(0),
        "new PDF during planning must replan, not omit:\n{}",
        format_output(&output)
    );
    let document = scan_json_stdout(&output);
    assert_eq!(document["summary"]["pdfs"], 2);
    assert_eq!(document["summary"]["created"], 2);
    assert!(env.vault.join("ref/blogs/first.md").is_file());
    assert!(env.vault.join("ref/blogs/second.md").is_file());
}

#[test]
fn same_length_edits_before_evidence_replan() {
    // Same-length sidecar/ref/config/parent edits after reads but before
    // evidence capture must invalidate: content hashes (not mtime/size)
    // plus membership are compared pre vs post inside preparation.
    let temp = TempDir::new("bob-cli-scan-interleave-before-evidence");
    let env = ScanEnv {
        vault: temp.path().join("vault"),
        state: temp.path().join("state"),
        lock: temp.path().join("bob_sync.lock"),
    };
    write_area_note(&env.vault.join("obsidian.md"), "Obsidian");
    let pdf = env.vault.join("lib/chat/live.pdf");
    write_textbundle_with_asset(&pdf, "Live", b"asset-v1");
    assert_success(&run_scan(&env, &[]));
    let note = env.vault.join("ref/chat/live.md");
    let sidecar = pdf.with_extension("textbundle").join("text.md");
    let sidecar_before = fs::read_to_string(&sidecar).expect("sidecar");
    assert_eq!(sidecar_before.len(), sidecar_before.len());
    // Same-length sidecar edit (change one char, keep length).
    let mut sidecar_chars: Vec<char> = sidecar_before.chars().collect();
    if let Some(first) = sidecar_chars.first_mut() {
        *first = if *first == '>' { '#' } else { '>' };
    }
    let sidecar_edited: String = sidecar_chars.into_iter().collect();
    assert_eq!(sidecar_edited.len(), sidecar_before.len());

    let hs = Handshake::new(temp.path(), "hs");
    let config = temp.path().join("config.yml");
    write_file(&config, "highlights:\n  foo: bar\n");
    let child = {
        let mut command = bob_command();
        command.arg("ref").arg("scan").arg("-f").arg("json");
        apply_scan_env(&mut command, &env)
            .env("BOB_CONFIG_FILE", &config)
            .env("BOB_REF_SCAN_TEST_PAUSE", "before_evidence")
            .env("BOB_REF_SCAN_TEST_READY", &hs.ready)
            .env("BOB_REF_SCAN_TEST_GO", &hs.go)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        SpawnedScan {
            child: Some(command.spawn().expect("spawn")),
        }
    };
    wait_pause(&hs, 1);
    // Same-length edits after reads, before evidence capture.
    fs::write(&sidecar, &sidecar_edited).expect("edit sidecar same length");
    let note_before = fs::read_to_string(&note).expect("note");
    // Same-length ref-note edit: flip one char, keep length.
    let mut note_chars: Vec<char> = note_before.chars().collect();
    if let Some(c) = note_chars.iter_mut().find(|c| **c == 'L') {
        *c = 'M';
    }
    let note_edited: String = note_chars.into_iter().collect();
    assert_eq!(note_edited.len(), note_before.len());
    fs::write(&note, &note_edited).expect("edit note same length");
    // Same-length config edit.
    fs::write(&config, "highlights:\n  foo: baz\n").expect("edit config");
    // Same-length parent edit (area note title).
    let parent_before =
        fs::read_to_string(env.vault.join("obsidian.md")).expect("parent");
    let parent_edited = parent_before.replacen("Obsidian", "ObsidiaN", 1);
    assert_eq!(parent_edited.len(), parent_before.len());
    fs::write(env.vault.join("obsidian.md"), &parent_edited)
        .expect("edit parent");
    // New unselected input: a fresh PDF the old plan did not select.
    write_marker_pdf(&env.vault.join("lib/chat/added.pdf"), "Added");
    resume(&hs, 1);
    let output = child.wait();
    // Must replan with current data or fail safely; never accept old work
    // with new evidence as stale no-work.
    assert!(
        output.status.code() == Some(0) || output.status.code() == Some(1),
        "same-length edits must replan or refuse:\n{}",
        format_output(&output)
    );
    let document = scan_json_stdout(&output);
    if document.get("error").is_some() {
        assert_eq!(document["error"]["code"], "scan_failed");
        assert!(
            document["error"]["message"]
                .as_str()
                .unwrap_or("")
                .contains("scan inputs changed"),
            "{}",
            format_output(&output)
        );
    } else {
        let created = document["summary"]["created"].as_u64().unwrap_or(0);
        let updated = document["summary"]["updated"].as_u64().unwrap_or(0);
        let failures = document["summary"]["failures"].as_u64().unwrap_or(0);
        assert!(
            created + updated + failures > 0,
            "edits must not be classified stale no-work:\n{}",
            format_output(&output)
        );
        assert!(
            env.vault.join("ref/chat/added.md").is_file()
                || document["summary"]["pdfs"].as_u64().unwrap_or(0) >= 1,
            "new input must be planned:\n{}",
            format_output(&output)
        );
    }
}

fn run_scan_extra(
    env: &ScanEnv,
    extra: &[&str],
    extra_env: &[(&str, &std::ffi::OsStr)],
) -> std::process::Output {
    let mut command = bob_command();
    command.arg("ref").arg("scan");
    for arg in extra {
        command.arg(arg);
    }
    apply_scan_env(&mut command, env);
    for (key, value) in extra_env {
        command.env(key, value);
    }
    command.output().expect("run ref scan")
}

fn same_length_replace(text: &str, from: &str, to: &str) -> String {
    let edited = text.replacen(from, to, 1);
    assert_eq!(
        edited.len(),
        text.len(),
        "replacement must keep the same length: {from:?} -> {to:?}"
    );
    edited
}

#[test]
fn text_markdown_edit_before_evidence_imports_or_refuses() {
    // Same-length text.markdown comment edit after planning reads and
    // before evidence capture. Uses an absolute lib path outside bob_dir
    // so vault-only Markdown enumeration cannot provide false coverage.
    let temp = TempDir::new("bob-cli-scan-interleave-text-markdown");
    let env = ScanEnv {
        vault: temp.path().join("vault"),
        state: temp.path().join("state"),
        lock: temp.path().join("bob_sync.lock"),
    };
    let lib = temp.path().join("external-lib");
    write_area_note(&env.vault.join("obsidian.md"), "Obsidian");
    write_marker_pdf(&lib.join("chat/stable.pdf"), "Stable");
    let pdf = lib.join("chat/live.pdf");
    write_textbundle(
        &pdf,
        "Live",
        b"asset-v1",
        "text.markdown",
        Some("Old content"),
    );
    let lib_env = [("BOB_HIGHLIGHTS_LIB_DIR", lib.as_os_str())];
    assert_success(&run_scan_extra(&env, &[], &lib_env));
    let note = env.vault.join("ref/chat/live.md");
    let stable = env.vault.join("ref/chat/stable.md");
    let before = fs::read_to_string(&note).expect("note");
    let stable_before = fs::read_to_string(&stable).expect("stable note");
    assert!(
        before.contains("Old content"),
        "initial import must include the sidecar comment:\n{before}"
    );
    let sidecar = pdf.with_extension("textbundle").join("text.markdown");
    let output =
        edit_at_pause(&env, temp.path(), "before_evidence", &lib_env, || {
            let body = fs::read_to_string(&sidecar).expect("sidecar");
            fs::write(
                &sidecar,
                same_length_replace(&body, "Old content", "New content"),
            )
            .expect("edit text.markdown");
        });
    assert_inputs_changed_or_imported(&output, &note, &before, "New content");
    assert_eq!(
        fs::read_to_string(&stable).expect("stable after"),
        stable_before,
        "no-work candidate must stay untouched"
    );
}

#[test]
fn text_md_edit_before_evidence_imports_or_refuses() {
    let temp = TempDir::new("bob-cli-scan-interleave-text-md");
    let env = ScanEnv {
        vault: temp.path().join("vault"),
        state: temp.path().join("state"),
        lock: temp.path().join("bob_sync.lock"),
    };
    write_area_note(&env.vault.join("obsidian.md"), "Obsidian");
    write_marker_pdf(&env.vault.join("lib/chat/stable.pdf"), "Stable");
    let pdf = env.vault.join("lib/chat/live.pdf");
    write_textbundle(&pdf, "Live", b"asset-v1", "text.md", Some("Old content"));
    assert_success(&run_scan(&env, &[]));
    let note = env.vault.join("ref/chat/live.md");
    let before = fs::read_to_string(&note).expect("note");
    let sidecar = pdf.with_extension("textbundle").join("text.md");
    let output =
        edit_at_pause(&env, temp.path(), "before_evidence", &[], || {
            let body = fs::read_to_string(&sidecar).expect("sidecar");
            fs::write(
                &sidecar,
                same_length_replace(&body, "Old content", "New content"),
            )
            .expect("edit text.md");
        });
    assert_inputs_changed_or_imported(&output, &note, &before, "New content");
}

#[test]
fn source_image_edit_before_evidence_copies_or_refuses() {
    let temp = TempDir::new("bob-cli-scan-interleave-source-image");
    let env = ScanEnv {
        vault: temp.path().join("vault"),
        state: temp.path().join("state"),
        lock: temp.path().join("bob_sync.lock"),
    };
    write_area_note(&env.vault.join("obsidian.md"), "Obsidian");
    let pdf = env.vault.join("lib/chat/live.pdf");
    write_textbundle_with_asset(&pdf, "Live", b"asset-v1");
    assert_success(&run_scan(&env, &[]));
    let note = env.vault.join("ref/chat/live.md");
    let before = fs::read_to_string(&note).expect("note");
    let source = pdf.with_extension("textbundle").join("assets/figure.png");
    let assets = env.vault.join("ref/chat/live.assets");
    let dest_before = first_asset_bytes(&assets);
    let output =
        edit_at_pause(&env, temp.path(), "before_evidence", &[], || {
            fs::write(&source, b"asset-v2").expect("edit source image");
        });
    assert!(
        output.status.code() == Some(0) || output.status.code() == Some(1),
        "{}",
        format_output(&output)
    );
    let document = scan_json_stdout(&output);
    if document.get("error").is_some() {
        assert_eq!(document["error"]["code"], "scan_failed");
        assert!(
            document["error"]["message"]
                .as_str()
                .unwrap_or("")
                .contains("scan inputs changed"),
            "{}",
            format_output(&output)
        );
        assert_eq!(fs::read_to_string(&note).expect("note"), before);
        assert_eq!(first_asset_bytes(&assets), dest_before);
        assert!(
            !asset_dir_contains(&assets, b"asset-v2"),
            "refusal must not copy the edited source"
        );
    } else {
        assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
        assert!(
            asset_dir_contains(&assets, b"asset-v2"),
            "replan must copy the current source asset:\n{}",
            format_output(&output)
        );
    }
}

#[test]
fn dest_asset_edit_before_evidence_restores_or_refuses() {
    let temp = TempDir::new("bob-cli-scan-interleave-dest-asset");
    let env = ScanEnv {
        vault: temp.path().join("vault"),
        state: temp.path().join("state"),
        lock: temp.path().join("bob_sync.lock"),
    };
    write_area_note(&env.vault.join("obsidian.md"), "Obsidian");
    let pdf = env.vault.join("lib/chat/live.pdf");
    write_textbundle_with_asset(&pdf, "Live", b"asset-v1");
    assert_success(&run_scan(&env, &[]));
    let note = env.vault.join("ref/chat/live.md");
    let before = fs::read_to_string(&note).expect("note");
    let assets = env.vault.join("ref/chat/live.assets");
    let dest = first_asset_path(&assets);
    let dest_before = fs::read(&dest).expect("dest bytes");
    assert_eq!(dest_before, b"asset-v1");
    let output =
        edit_at_pause(&env, temp.path(), "before_evidence", &[], || {
            fs::write(&dest, b"asset-vX").expect("edit dest asset");
        });
    assert!(
        output.status.code() == Some(0) || output.status.code() == Some(1),
        "{}",
        format_output(&output)
    );
    let document = scan_json_stdout(&output);
    let dest_after = fs::read(&dest).expect("dest after");
    if document.get("error").is_some() {
        assert_eq!(document["error"]["code"], "scan_failed");
        assert!(
            document["error"]["message"]
                .as_str()
                .unwrap_or("")
                .contains("scan inputs changed"),
            "{}",
            format_output(&output)
        );
        assert_eq!(fs::read_to_string(&note).expect("note"), before);
        assert_eq!(dest_after, b"asset-vX");
    } else {
        assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
        assert_eq!(
            dest_after,
            b"asset-v1",
            "replan must restore dest from the unchanged source:\n{}",
            format_output(&output)
        );
    }
}

#[test]
fn pdf_added_after_snapshot_replans_with_both_or_refuses() {
    // Membership at the discovery/snapshot boundary: the planned PDF list
    // is derived from the membership snapshot, so a PDF planted after that
    // snapshot must invalidate instead of being omitted.
    let temp = TempDir::new("bob-cli-scan-interleave-after-snapshot");
    let env = ScanEnv {
        vault: temp.path().join("vault"),
        state: temp.path().join("state"),
        lock: temp.path().join("bob_sync.lock"),
    };
    write_area_note(&env.vault.join("obsidian.md"), "Obsidian");
    write_marker_pdf(&env.vault.join("lib/chat/stable.pdf"), "Stable");
    assert_success(&run_scan(&env, &[]));
    let stable = env.vault.join("ref/chat/stable.md");
    let stable_before = fs::read_to_string(&stable).expect("stable");
    let added = env.vault.join("lib/chat/added.pdf");
    let added_note = env.vault.join("ref/chat/added.md");
    let output =
        edit_at_pause(&env, temp.path(), "after_snapshot", &[], || {
            write_marker_pdf(&added, "Added");
        });
    assert!(
        output.status.code() == Some(0) || output.status.code() == Some(1),
        "{}",
        format_output(&output)
    );
    let document = scan_json_stdout(&output);
    if document.get("error").is_some() {
        assert_eq!(document["error"]["code"], "scan_failed");
        assert!(
            document["error"]["message"]
                .as_str()
                .unwrap_or("")
                .contains("scan inputs changed"),
            "{}",
            format_output(&output)
        );
        assert!(!added_note.exists(), "refusal must not create the new note");
        assert_eq!(fs::read_to_string(&stable).expect("stable"), stable_before);
    } else {
        assert_eq!(output.status.code(), Some(0), "{}", format_output(&output));
        assert_eq!(document["summary"]["pdfs"], 2);
        assert!(
            added_note.is_file(),
            "replan must include the PDF that arrived at the snapshot boundary:\n{}",
            format_output(&output)
        );
        assert_eq!(
            fs::read_to_string(&stable).expect("stable after"),
            stable_before
        );
    }
}

fn first_asset_path(dir: &Path) -> PathBuf {
    let mut files = fs::read_dir(dir)
        .expect("assets dir")
        .filter_map(|entry| {
            let entry = entry.ok()?;
            entry.file_type().ok()?.is_file().then(|| entry.path())
        })
        .collect::<Vec<_>>();
    files.sort();
    files.into_iter().next().unwrap_or_else(|| {
        panic!("expected an image asset in {}", dir.display())
    })
}

fn first_asset_bytes(dir: &Path) -> Vec<u8> {
    fs::read(first_asset_path(dir)).expect("read dest asset")
}

fn asset_dir_contains(dir: &Path, bytes: &[u8]) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        fs::read(entry.path()).is_ok_and(|contents| contents == bytes)
    })
}

#[test]
fn queued_queued_output_collision_rejects_without_publication() {
    // Two queued PDFs mapping to one ref note must be rejected as dry-run
    // does, never overwriting one reference with the other.
    let temp = TempDir::new("bob-cli-scan-interleave-queued-queued");
    let env = ScanEnv {
        vault: temp.path().join("vault"),
        state: temp.path().join("state"),
        lock: temp.path().join("bob_sync.lock"),
    };
    write_marker_pdf(&env.vault.join("xlib/blogs/clash.pdf"), "First");
    write_marker_pdf(&env.vault.join("xlib/blogs/clash.PDF"), "Second");
    let dry = run_scan(&env, &["-f", "json", "--dry-run"]);
    assert_eq!(dry.status.code(), Some(1), "{}", format_output(&dry));
    assert_eq!(scan_json_stdout(&dry)["error"]["code"], "output_collision");
    let output = run_scan(&env, &["-f", "json"]);
    assert_eq!(
        output.status.code(),
        Some(1),
        "writing scan must reject the collision:\n{}",
        format_output(&output)
    );
    let document = scan_json_stdout(&output);
    assert_eq!(document["error"]["code"], "output_collision");
    assert_eq!(
        document["error"]["paths"],
        serde_json::json!(["ref/blogs/clash.md"])
    );
    // Never overwrites one reference with the other: no moves, no notes.
    assert!(env.vault.join("xlib/blogs/clash.pdf").is_file());
    assert!(env.vault.join("xlib/blogs/clash.PDF").is_file());
    assert!(!env.vault.join("lib/blogs/clash.pdf").exists());
    assert!(!env.vault.join("lib/blogs/clash.PDF").exists());
    assert!(!env.vault.join("ref/blogs/clash.md").exists());
}

#[test]
fn queued_existing_output_collision_rejects_without_overwrite() {
    // A queued PDF colliding with an existing library PDF's note (different
    // case, same ref note) must be rejected before any move, leaving the
    // existing reference untouched.
    let temp = TempDir::new("bob-cli-scan-interleave-queued-existing");
    let env = ScanEnv {
        vault: temp.path().join("vault"),
        state: temp.path().join("state"),
        lock: temp.path().join("bob_sync.lock"),
    };
    write_marker_pdf(&env.vault.join("lib/blogs/clash.PDF"), "Existing");
    assert_success(&run_scan(&env, &[]));
    let note = env.vault.join("ref/blogs/clash.md");
    assert!(note.is_file());
    let note_before = fs::read(&note).expect("existing note");
    let receipt_before =
        fs::read(scan_receipt_path(&env.state)).expect("receipt");
    write_marker_pdf(&env.vault.join("xlib/blogs/clash.pdf"), "Queued");
    let output = run_scan(&env, &["-f", "json"]);
    assert_eq!(
        output.status.code(),
        Some(1),
        "queued/existing collision must fail:\n{}",
        format_output(&output)
    );
    assert_eq!(
        scan_json_stdout(&output)["error"]["code"],
        "output_collision"
    );
    assert_eq!(fs::read(&note).expect("note after"), note_before);
    assert!(env.vault.join("xlib/blogs/clash.pdf").is_file());
    assert!(!env.vault.join("lib/blogs/clash.pdf").exists());
    assert_eq!(
        fs::read(scan_receipt_path(&env.state)).expect("receipt after"),
        receipt_before,
        "prelock collision must leave the receipt untouched"
    );
}

#[test]
fn recovery_then_no_work_publishes_receipt() {
    // Recovery inside the mutation window must settle its receipt while
    // serialized: after recovery, a no-work rebuild must not leave the
    // sentinel receipt untouched.
    let (temp, vault, state) =
        overwrite_recovery::fixture("bob-cli-scan-interleave-recovery-nowork");
    let env = ScanEnv {
        vault: vault.clone(),
        state: state.clone(),
        lock: temp.path().join("bob_sync.lock"),
    };
    write_marker_pdf(&vault.join("lib/blogs/stable.pdf"), "Stable");
    let scan_env = ScanEnv {
        vault: vault.clone(),
        state: state.clone(),
        lock: temp.path().join("bob_sync.lock"),
    };
    assert_success(&run_scan(&scan_env, &[]));
    let receipt_before =
        fs::read(scan_receipt_path(&state)).expect("initial receipt");
    // Queue work so preparation sees mutation, then remove it during the
    // pause while planting a valid overwrite intent. Recovery will mutate
    // (publishing a receipt) while the rebuild finds no-work.
    write_marker_pdf(&vault.join("xlib/blogs/queued.pdf"), "Queued");
    let base = overwrite_recovery::head(&vault);
    let hs = Handshake::new(temp.path(), "hs");
    let child =
        spawn_scan(&env, &["-f", "json"], &hs, "after_prepare", false, None);
    wait_pause(&hs, 1);
    fs::remove_file(vault.join("xlib/blogs/queued.pdf")).expect("remove work");
    fs::write(
        vault.join("ref/blogs/half-written.md"),
        "---\nid: half-written\n---\nCRASHED\n",
    )
    .expect("crash note");
    overwrite_recovery::write_intent(
        &state,
        &vault,
        &base,
        None,
        &["ref/blogs/half-written.md"],
        &[],
    );
    resume(&hs, 1);
    let output = child.wait();
    assert_eq!(
        output.status.code(),
        Some(0),
        "recovery then no-work must succeed:\n{}",
        format_output(&output)
    );
    let document = scan_json_stdout(&output);
    assert_eq!(document["ok"], true);
    let receipt_after =
        fs::read(scan_receipt_path(&state)).expect("receipt after recovery");
    assert_ne!(
        receipt_after, receipt_before,
        "recovery must replace the sentinel receipt before no-work return"
    );
    assert!(
        !overwrite_recovery::intent_path(&state, &vault).exists(),
        "intent is cleared"
    );
}

#[test]
fn post_recovery_replan_failure_keeps_direct_output() {
    // When recovery succeeds but the rebuild fails (e.g., a collision
    // planted during the pause), direct output must report the replan
    // failure while the receipt stays as the serialized recovery result.
    let (temp, vault, state) =
        overwrite_recovery::fixture("bob-cli-scan-interleave-recovery-fail");
    let env = ScanEnv {
        vault: vault.clone(),
        state: state.clone(),
        lock: temp.path().join("bob_sync.lock"),
    };
    write_marker_pdf(&vault.join("lib/blogs/stable.pdf"), "Stable");
    let scan_env = ScanEnv {
        vault: vault.clone(),
        state: state.clone(),
        lock: temp.path().join("bob_sync.lock"),
    };
    assert_success(&run_scan(&scan_env, &[]));
    write_marker_pdf(&vault.join("xlib/blogs/queued.pdf"), "Queued");
    let base = overwrite_recovery::head(&vault);
    let hs = Handshake::new(temp.path(), "hs");
    let child =
        spawn_scan(&env, &["-f", "json"], &hs, "after_prepare", false, None);
    wait_pause(&hs, 1);
    fs::write(
        vault.join("ref/blogs/half-written.md"),
        "---\nid: half-written\n---\nCRASHED\n",
    )
    .expect("crash note");
    overwrite_recovery::write_intent(
        &state,
        &vault,
        &base,
        None,
        &["ref/blogs/half-written.md"],
        &[],
    );
    // Plant a queued-queued output collision so the post-recovery rebuild
    // fails.
    write_marker_pdf(&vault.join("xlib/blogs/clash.pdf"), "First");
    write_marker_pdf(&vault.join("xlib/blogs/clash.PDF"), "Second");
    resume(&hs, 1);
    let output = child.wait();
    assert_eq!(
        output.status.code(),
        Some(1),
        "post-recovery replan failure:\n{}",
        format_output(&output)
    );
    let document = scan_json_stdout(&output);
    assert_eq!(document["ok"], false);
    assert_eq!(document["error"]["code"], "output_collision");
    // Receipt must be the serialized recovery result, not a later unlocked
    // write and not the stale sentinel: it exists and parses, and the
    // intent is cleared by the serialized recovery.
    let receipt = fs::read_to_string(scan_receipt_path(&state))
        .expect("recovery receipt survives replan failure");
    let receipt_doc: Value =
        serde_json::from_str(&receipt).expect("receipt json");
    assert_eq!(receipt_doc["ok"], true);
    assert!(
        !overwrite_recovery::intent_path(&state, &vault).exists(),
        "intent is cleared by serialized recovery"
    );
}
